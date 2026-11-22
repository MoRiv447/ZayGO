//! 三个负载生成器 —— 实验的自变量。
//!
//! 为什么必须区分 `Spin` 和 `Web`：纯死循环是**最好情况** ——
//! 没有内存访问、没有分支预测失败、不占总线带宽。
//! 真实的 HTTP 解析 + 帧缓冲填充会触发 cache miss 和总线争用，
//! 干扰会明显更大。只测死循环会低估风险。

use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use crate::crit;
use crate::sched;
use crate::time;

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    /// 实验 01 L-A：极轻空转（只有取指与分支，无内存访问）。
    Idle = 0,
    /// 实验 01 L-B：纯算术死循环，不碰内存。
    Spin = 1,
    /// 实验 01 L-C：分支密集的"解析" + 写内存的"渲染"。
    Web = 2,
    /// 实验 02：在临界区里屏蔽中断 M 个周期，M 按 [`MASKS`] 逐档扫描。
    Mask = 3,
    /// P1：串口 CLI。控制台跑在 LOAD 任务上 —— 它是"不可信负载"，
    /// 慢、可能阻塞，但优先级最低，碰不到实时面。
    Cli = 4,
}

/// 实验 02 的自变量：临界区屏蔽时长（周期数）。
///
/// 选点依据：dispatch 约 230 周期、周期 64000 周期。
/// 500 起跳是为了避开 230 这个底噪；1000~16000 是"延迟明显但还不丢 tick"的主区间；
/// 96000 超过一整周期 —— 最后一档专门用来暴露**丢 tick** 的现象。
pub const MASKS: [u32; 8] = [0, 500, 1000, 2000, 4000, 8000, 16000, 96000];

static MODE: AtomicU8 = AtomicU8::new(0);
static STAGE: AtomicUsize = AtomicUsize::new(0);

pub fn set_mode(m: Mode) {
    MODE.store(m as u8, Ordering::Relaxed);
}

fn mode() -> Mode {
    match MODE.load(Ordering::Relaxed) {
        1 => Mode::Spin,
        2 => Mode::Web,
        3 => Mode::Mask,
        4 => Mode::Cli,
        _ => Mode::Idle,
    }
}

/// 是否是交互式 CLI 模式。
///
/// 用它把调度器里的"采满就 dump 并停机"逻辑挡住 —— CLI 要一直活着，
/// 不能因为采样缓冲满了就退出。
pub fn is_cli() -> bool {
    mode() == Mode::Cli
}

/// 是否处于屏蔽扫描模式。调度器用它决定采满一档后要不要换档。
pub fn is_sweep() -> bool {
    mode() == Mode::Mask
}

pub fn stage() -> usize {
    STAGE.load(Ordering::Relaxed).min(MASKS.len() - 1)
}

pub fn current_mask() -> u32 {
    MASKS[stage()]
}

/// 推进到下一档。返回 `false` 表示已经是最后一档。
pub fn advance_stage() -> bool {
    let s = STAGE.load(Ordering::Relaxed);
    if s + 1 >= MASKS.len() {
        false
    } else {
        STAGE.store(s + 1, Ordering::Relaxed);
        true
    }
}

/// 低优先级任务每轮调用一次。单轮要短（几千周期），
/// 这样抢占点足够密集，才能测到真实的响应延迟分布。
#[inline(never)]
pub fn load_iteration() {
    match mode() {
        Mode::Idle => {
            // 原本这里用 `wfi()` 表示"完全空闲"。实测这条路走不通：
            // 核心一进 Sleep，SWD 调试时钟就停，probe-rs 立刻报
            // "Target device responded with a FAULT response" 并失联，
            // 连 `probe-rs read` 都读不到，只能 connect-under-reset 救回来。
            //
            // 已试过置 DBGMCU_CR.DBG_SLEEP=1（见 main.rs 的 keep_debug_in_sleep），
            // 仍未解决 —— 具体原因待查，可能是 H7 的 D3 域调试时钟另有开关。
            //
            // 改用极轻空转，三个理由：
            // 1) 不产生内存访问、不占总线，对实时任务的干扰最小；
            // 2) 反而更纯粹 —— 排除了 WFI 唤醒延迟，测到的就是调度本身的开销；
            // 3) 三种模式因此都是"忙等"，对比是同口径的，不会把休眠/唤醒的
            //    差异混进"负载差异"里。
            //
            // 代价：这不是真正的"空闲系统"，功耗也不为零。作为调度延迟基线够用。
            for _ in 0..8 {
                cortex_m::asm::nop();
            }
            core::hint::black_box(());
        }
        Mode::Spin => spin_round(),
        Mode::Web => web_round(),
        Mode::Mask => mask_round(),
        Mode::Cli => crate::cli::poll(),
    }
}

/// 实验 02 的核心：制造「释放点被屏蔽 M 个周期」这个工况。
///
/// `cortex_m::interrupt::free` 在单核 Cortex-M 上就是 `cpsid i` … `cpsie i`。
/// 屏蔽期间 SysTick 无法进入，pending 位被置起，等 `cpsie i` 后立刻补进 ——
/// 这正是真实系统里"关中断过长导致响应延迟"的等价场景。
///
/// # 为什么要把临界区对齐到释放点
///
/// 屏蔽起点若是随机的，屏蔽 M 周期只有 `M / 64000` 的概率盖住释放点：
/// M=500 时是 0.8%，要几千个样本才碰得上一次，测出来的 max 全是噪声。
/// 所以这里刻意构造**最坏情况** —— 而最坏情况正是我们要测的东西。
///
/// # 为什么不能靠"等到快释放时再关中断"
///
/// 第一版就是那么写的：等到距释放点还剩 16 个周期时调 `interrupt::free`。
/// 结果全部档位的延迟都是 0 —— 因为 `cpsid i` 的调用开销约 20 周期，
/// 等中断真正关掉时释放点已经过去了。**用软件去追硬件边界，追不上。**
///
/// 所以改成：提前 `PRE` 个周期关中断，然后**在关中断状态下**忙等到相位回绕
/// （即释放点已过），再多屏蔽 M 个周期才开。这样被屏蔽的时长精确等于 M，
/// 与 `cpsid` 的开销无关。
///
/// 代价：临界区总时长是 `PRE + M`，不是 M。对外报告的是 M，即"释放点被推迟多长"。
///
/// # 为什么 M=0 也走同一条路径
///
/// 第一版让 M=0 直接返回、完全不进临界区，结果它的 phase（218）和其余档位的
/// 偏移（约 390）对不上 —— 基线不齐，曲线就不是一条直线。
/// 现在 M=0 也经历"预窗口 + 等回绕 + 屏蔽 0 周期"，于是 phase = M + 常数 恒成立。
///
/// 绝对底噪（完全没有临界区的场景）见实验 01：约 226 周期。
#[inline(never)]
fn mask_round() {
    let m = current_mask();

    let period = time::cycles_per_ms();
    let origin = sched::origin();
    /// 提前量。必须明显大于 `cpsid i` 的调用开销（实测约 20 周期），
    /// 但又要远小于最小的屏蔽时长（500），否则小档位会被提前量淹没。
    const PRE: u32 = 200;

    if m < period {
        // 1) 等到距离释放点还剩 PRE 个周期
        loop {
            let p = time::now().wrapping_sub(origin) % period;
            if p >= period - PRE {
                break;
            }
        }

        // 2) 关中断。在中断已关的状态下等相位回绕，再多屏蔽 M 个周期。
        //    注意：等回绕的这段也在屏蔽，所以相位一旦回绕就说明释放点已被挡住。
        crit::mask(crit::SITE_SWEEP_MASK_STIMULUS, || {
            let p0 = time::now().wrapping_sub(origin) % period;
            loop {
                let p = time::now().wrapping_sub(origin) % period;
                if p < p0 {
                    break; // 相位回绕 = 释放点已过
                }
            }
            let t = time::now();
            while time::elapsed(t) < m {
                core::hint::spin_loop();
            }
        });
    } else {
        // 超过一个周期：直接整段屏蔽，让 tick 被丢掉 —— 专门用来暴露这个现象
        crit::mask(crit::SITE_SWEEP_MASK_STIMULUS, || {
            let t = time::now();
            while time::elapsed(t) < m {
                core::hint::spin_loop();
            }
        });
    }

    core::hint::black_box(m);
}

/// 纯寄存器运算。刻意不加内存访问，用来分离"调度开销"与"内存/总线干扰"。
#[inline(never)]
fn spin_round() {
    let mut a = 0x1234_5678u32;
    let mut b = 0x9ABC_DEF0u32;
    for _ in 0..512 {
        a = a.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        b ^= a.rotate_left(7);
        a = a.wrapping_add(b >> 3);
    }
    // black_box 防止整个循环被优化掉 —— 否则测的是空循环
    core::hint::black_box((a, b));
}

static mut BUF: [u8; 256] = [0; 256];
static mut FB: [u32; 128] = [0; 128];

/// 模拟管理面负载：读缓冲（分支密集）+ 写帧缓冲（占内存带宽）。
#[inline(never)]
fn web_round() {
    unsafe {
        let buf = &raw mut BUF as *mut u8;
        let mut tokens = 0u32;
        let mut sum = 0u32;
        for i in 0..256 {
            let b = buf.add(i).read();
            if b == b' ' || b == b'\n' || b == b',' {
                tokens += 1;
            }
            sum = sum.wrapping_add(b as u32);
        }

        let fb = &raw mut FB as *mut u32;
        for i in 0..128 {
            fb.add(i).write(sum ^ (i as u32).wrapping_mul(2_654_435_761));
        }

        core::hint::black_box((tokens, sum));
    }
}

static RT_DATA: [u32; 32] = [0x1111_1111; 32];

/// 实时任务的固定工作量。保持很小：本实验关心的是"什么时候轮到它跑"，
/// 不是"它跑多久"。若这里太重，会引入超期，混淆两个不同的指标。
#[inline(never)]
pub fn rt_work() {
    let mut acc = 0u32;
    for v in RT_DATA.iter() {
        acc = acc.wrapping_add(*v);
    }
    core::hint::black_box(acc);
}
