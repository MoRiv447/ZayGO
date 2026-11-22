//! 采样与统计。
//!
//! # 为什么实验 02 必须换指标
//!
//! 实验 01 的核心指标是 `t_start - t_release`。但屏蔽中断时 SysTick 只是被**推迟**：
//! 等 `cpsie i` 之后才进 ISR，`t_release` 跟着一起后移，两者相减仍是 230 周期上下。
//! **用旧指标测屏蔽，会得出"零干扰"的错误结论。**
//!
//! 所以这里同时记录两个量：
//!
//! - `phase`    = `t_start % PERIOD_CYCLES` —— 相对**理想**释放时刻的偏移
//! - `dispatch` = `t_start - t_release`     —— ISR 入口到任务开始
//!
//! "理想释放时刻"由硬件周期决定（CYCCNT 在 `time::init()` 里清零，故理想点就是
//! PERIOD 的整数倍），不会跟着屏蔽一起漂。于是：
//!
//! - 屏蔽刚好盖住释放点 → `phase ≈ 屏蔽时长 + dispatch`，`dispatch` 不变
//! - 没盖住            → `phase ≈ dispatch`
//!
//! 多个样本取 max，就得到"屏蔽 M 周期 → 最坏延迟逼近 M"的直接证据。
//! `dispatch` 不随 M 变化这件事本身也是结论：屏蔽伤害的是"何时开始响应"，
//! 不是"切换本身有多贵"。

use rtt_target::rprintln;

use crate::cache;
use crate::crit;
use crate::load;
use crate::sched;
use crate::time;

/// 每档样本数。
///
/// 因为临界区已对齐到释放点（见 `load::mask_round`），每个样本都会命中，
/// 不像随机相位那样需要几千个样本才碰上一次 —— 128 足够看清 max。
/// 而且 96000 周期那一档每个样本要 96ms，样本多了总时长会爆。
pub const STAGES: usize = 8;
pub const PER_STAGE: usize = 128;
pub const CAP: usize = STAGES * PER_STAGE;

/// 结果块。放在 `.uninit` 段 —— cortex-m-rt 只清零 `.bss`，`NOLOAD` 的 `.uninit`
/// 会被原样保留。这是"高时钟测完、复位后再输出"这套绕法的物理基础。
///
/// 跨复位存活的前提：`SYSRESETREQ` 是**系统复位**，除备份域外 SRAM 内容保留；
/// 上电复位才会清空。所以不能拔电。
#[repr(C)]
struct Block {
    /// 时钟配置的进度标记。
    ///
    /// **刻意放在偏移 0**：这样它的地址就是 `.uninit` 的起始地址，
    /// 可以在固件挂死、调试口全废的情况下，靠"擦 flash → 读 RAM"取出来。
    /// 这是唯一一种不依赖 SWD 实时连接的取证手段。
    progress: [u32; 16],
    /// 有效标志。值为 [`MAGIC`] 才认为块里是完整结果。
    magic: u32,
    /// 样本数。
    n: u32,
    /// 测量时的 CPU 频率 —— 复位后时钟已回到 HSI，换算必须用这个值。
    cpu_hz: u32,
    /// 哪种实验，见 MODE_SWEEP / MODE_SINGLE。
    mode: u32,
    /// 屏蔽预算站点的实测副本。复位后 SITE_MAX 那些 normal RAM 已清零，
    /// 所以必须随块一起存。
    site_max: [u32; 4],
    site_viol: [u32; 4],
    dispatch: [u32; CAP],
    phase: [u32; CAP],
    period: [u32; CAP],
}

const MAGIC: u32 = 0x5A41_5947; // "ZAYG"，随手取的，只为能一眼认出来

#[link_section = ".uninit"]
static mut BLOCK: core::mem::MaybeUninit<Block> = core::mem::MaybeUninit::uninit();

#[inline(always)]
fn block() -> &'static mut Block {
    unsafe { &mut *BLOCK.as_mut_ptr() }
}

/// 当前已记录样本数。
static mut N: usize = 0;
/// 上一条样本的 `t_start`，用于算周期抖动。
static mut PREV_START: u32 = 0;
static mut HAS_PREV: bool = false;

pub fn record(t_start: u32, t_release: u32) {
    // 写入三条数组 + 推进索引是一个真实的临界区：中断若在写到一半时进来，
    // 读到的就是半成品。用带预算申报的包装，顺便让 sample_ring_push 站点
    // 有真实负载 —— 声明了却从不执行的站点，运行期最大值恒为 0，等于没验证。
    crit::mask(crit::SITE_SAMPLE_RING_PUSH, || unsafe {
        if N >= CAP {
            return;
        }
        let dispatch = t_start.wrapping_sub(t_release);
        // 理想释放点是 `origin + k * PERIOD`，不是 `k * PERIOD` ——
        // CYCCNT 清零与 SysTick 使能之间有几百周期的间隔，见 sched::ORIGIN。
        let phase = t_start.wrapping_sub(sched::origin()) % time::cycles_per_ms();
        let period = if HAS_PREV {
            t_start.wrapping_sub(PREV_START)
        } else {
            0
        };

        let b = block();
        b.dispatch[N] = dispatch;
        b.phase[N] = phase;
        b.period[N] = period;

        HAS_PREV = true;
        PREV_START = t_start;
        N += 1;
    });
}

pub fn len() -> usize {
    unsafe { N }
}

/// 当前这一档是否已采满。
///
/// 用 `N % PER_STAGE` 判断，不需要单独记阶段号：只要每档样本数固定，
/// 索引本身就编码了阶段。
pub fn is_stage_full() -> bool {
    unsafe { N > 0 && N % PER_STAGE == 0 }
}

/// 高于这个频率就不再信任 RTT。
///
/// 实测：切到 PLL 550MHz 后调试口会变成 `WAIT response`，而且不确定复现
/// （同一固件两次运行死在不同步）。所以高时钟下不用 RTT，改存盘 + 复位。
const SAFE_RTT_HZ: u32 = 100_000_000;

/// 本次运行是哪种实验。存进结果块，复位后才知道该按哪种格式输出。
const MODE_SINGLE: u32 = 0;
const MODE_SWEEP: u32 = 1;

/// 结束一次运行。
///
/// 分两条路：
///
/// - **低时钟**（HSI 64MHz）：直接 `rprintln!` 输出，和以前一样。
/// - **高时钟**（PLL）：RTT 不可靠，所以把结果锁进 `.uninit`，触发系统复位；
///   复位后芯片回到 HSI，调试口恢复，由 [`dump_stored`] 把数据打出来。
///
/// 这不是"绕过问题"，是把 **测量** 和 **传输** 解耦：周期数是在目标频率下测的，
/// 传输走的是复位后的安全频率。
fn finish(sweep: bool) -> ! {
    // stop_tick 内部会清 D-Cache 并关掉它 —— 之后所有写都直接落到 RAM，
    // 所以下面写 magic 的顺序是安全的，不会被留在 cache 里丢掉。
    stop_tick();

    if time::cpu_hz() > SAFE_RTT_HZ {
        // 高时钟：先把结果锁进 .uninit，再复位，让芯片回到能调试的频率
        store(sweep);
        system_reset();
    }

    // 低时钟：直接输出
    if sweep {
        print_sweep(unsafe { N }, time::cpu_hz());
    } else {
        print_single(unsafe { N });
    }
    // 打完就清掉，否则下次上电会误认为"还有一份结果没输出"。
    // 这不是理论问题：第一次实现漏了这一步，结果下一轮烧录后固件直接
    // 输出了上一轮的旧数据，还以为是新测的。
    reset();
    halt()
}

/// 把当前结果锁进 `.uninit`。
fn store(sweep: bool) {
    unsafe {
        let b = block();
        b.n = N as u32;
        b.cpu_hz = time::cpu_hz();
        b.mode = if sweep { MODE_SWEEP } else { MODE_SINGLE };
        for i in 0..4 {
            b.site_max[i] = if i < crit::site_count() {
                crit::SITE_MAX[i].load(core::sync::atomic::Ordering::Relaxed)
            } else {
                0
            };
            b.site_viol[i] = if i < crit::site_count() {
                crit::SITE_VIOLATIONS[i].load(core::sync::atomic::Ordering::Relaxed)
            } else {
                0
            };
        }
        // magic 最后写：它是"这块内容有效"的唯一判据，必须最后落地
        b.magic = MAGIC;
    }
}

/// 写一个进度标记。**不打印、不依赖调试口**，直接落 RAM。
///
/// 用于时钟配置这类"可能把调试口一起搞死"的代码：出了问题之后，
/// 擦掉 flash 再读 `.uninit` 起始的 16 个字，就知道执行到第几步。
#[inline(always)]
pub fn mark(step: usize, value: u32) {
    if step < 16 {
        unsafe { block().progress[step] = value };
    }
}

/// `.uninit` 块的起始地址，供 host 端读进度用。
pub fn block_addr() -> u32 {
    unsafe { core::ptr::addr_of!(BLOCK) as u32 }
}

/// 清空结果块与计数器。每次真正开始测量前必须调用。
pub fn reset() {
    unsafe {
        let b = block();
        for i in 0..16 {
            b.progress[i] = 0;
        }
        b.magic = 0;
        b.n = 0;
        N = 0;
        HAS_PREV = false;
        PREV_START = 0;
    }
}

/// 触发系统复位。
///
/// `SYSRESETREQ` 是 **系统复位**，SRAM 内容保留（上电复位才会清）。
/// 所以结果块能活下来 —— 前提是不能拔电。
fn system_reset() -> ! {
    const AIRCR: *mut u32 = 0xE000_ED0C as *mut u32;
    unsafe {
        core::arch::asm!("dsb");
        // VECTKEY = 0x05FA，SYSRESETREQ = bit2
        AIRCR.write_volatile(0x05FA_0004);
        loop {
            core::arch::asm!("nop");
        }
    }
}

/// 复位后是否有一份完整结果等着输出。
pub fn has_results() -> bool {
    unsafe {
        let b = block();
        b.magic == MAGIC && b.n as usize <= CAP && b.n > 0
    }
}

/// 复位后：把存下的结果用 RTT 打出来，然后停机。
pub fn dump_stored() -> ! {
    let (n, hz, mode) = unsafe {
        let b = block();
        (b.n as usize, b.cpu_hz, b.mode)
    };

    rprintln!(
        "# restored from .uninit after reset: n={} measured_at={} Hz (now {} Hz)",
        n,
        hz,
        time::cpu_hz()
    );

    if mode == MODE_SWEEP {
        print_sweep(n, hz);
    } else {
        print_single(n);
    }

    // 清掉 magic：否则下一次复位会重复输出同一份数据
    unsafe {
        block().magic = 0;
    }
    halt()
}

/// 实验 02：按屏蔽时长分档输出。
///
/// 列：`stage,mask_cycles,idx,phase_cycles,dispatch_cycles`
fn print_sweep(n: usize, hz: u32) {
    rprintln!(
        "# exp02 interrupt-masking sweep: samples={} stages={} per_stage={} at={} Hz",
        n, STAGES, PER_STAGE, hz
    );
    rprintln!("stage,mask_cycles,idx,phase_cycles,dispatch_cycles");

    let b = block();
    for i in 0..n {
        let st = i / PER_STAGE;
        rprintln!(
            "{},{},{},{},{}",
            st,
            load::MASKS[st],
            i % PER_STAGE,
            b.phase[i],
            b.dispatch[i]
        );
    }

    rprintln!(
        "# overruns={} tick_missed={}",
        sched::OVERRUN.load(core::sync::atomic::Ordering::Relaxed),
        sched::TICK_MISSED.load(core::sync::atomic::Ordering::Relaxed)
    );
    dump_mask_budget();
    rprintln!("# END");
}

/// 中断屏蔽预算的运行期实测报告。
///
/// 和构建期报告对照着看：构建期检查的是 **申报值**，这里检查的是 **实测值**。
/// 两者都通过，才能说"这一版代码的屏蔽时长在预算内"——但仍然不是 WCET 证明。
pub fn dump_mask_budget() {
    rprintln!(
        "# mask budget: declared_max={} cycles",
        crit::MAX_MASK_CYCLES
    );
    rprintln!("# site,budget,observed_max,violations");
    let b = block();
    let stored = b.magic == MAGIC;
    for i in 0..crit::site_count() {
        // 复位后的路径用块里存的副本（SITE_MAX 在 normal RAM 里已清零）；
        // 同一次运行里 magic 也是刚写上的，两者取值一致。
        let obs = if stored {
            b.site_max[i]
        } else {
            crit::SITE_MAX[i].load(core::sync::atomic::Ordering::Relaxed)
        };
        let vio = if stored {
            b.site_viol[i]
        } else {
            crit::SITE_VIOLATIONS[i].load(core::sync::atomic::Ordering::Relaxed)
        };
        rprintln!(
            "# {},{},{},{}",
            crit::SITE_NAMES[i],
            crit::SITE_BUDGETS[i],
            obs,
            vio
        );
    }
}

/// 实验 01：单档分布输出。
///
/// 列：`idx,latency_cycles,period_cycles`（`latency` 就是 `dispatch`）
fn print_single(n: usize) {
    rprintln!("# exp01 samples n={}", n);
    rprintln!("idx,latency_cycles,period_cycles");

    let b = block();
    let mut min = u32::MAX;
    let mut max = 0u32;
    let mut sum: u64 = 0;

    for i in 0..n {
        let l = b.dispatch[i];
        if l < min {
            min = l;
        }
        if l > max {
            max = l;
        }
        sum += l as u64;
        rprintln!("{},{},{}", i, l, b.period[i]);
    }

    let mean = if n > 0 { (sum / n as u64) as u32 } else { 0 };
    rprintln!(
        "# min={} max={} mean={} overruns={}",
        min,
        max,
        mean,
        sched::OVERRUN.load(core::sync::atomic::Ordering::Relaxed)
    );
    rprintln!("# END");
}

/// 对外的两个入口，保持调度器那边的调用点不变。
pub fn dump_sweep() -> ! {
    finish(true)
}

pub fn dump_single() -> ! {
    finish(false)
}

/// 停 SysTick：否则中断会打断输出，而且此时已经采满，继续释放只会污染数据。
fn stop_tick() {
    unsafe {
        (0xE000_E010 as *mut u32).write_volatile(0);
    }
    // 关键：dump 前必须把 D-Cache 清干净再关掉。
    // 否则 rprintln! 写进 cache 的字符脏在 cache 里，probe-rs 通过 SWD 读 RAM
    // 只能读到旧数据 —— 表现为"输出莫名缺行"，且不报任何错。
    crate::cache::prepare_for_rtt();
}

/// 用 bkpt 结束，作为"我跑完了"的信号。probe-rs 会因此以非零码退出，属正常。
fn halt() -> ! {
    loop {
        cortex_m::asm::bkpt();
    }
}
