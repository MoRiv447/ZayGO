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

static mut DISPATCH: [u32; CAP] = [0; CAP];
static mut PHASE: [u32; CAP] = [0; CAP];
static mut PERIOD: [u32; CAP] = [0; CAP];
static mut N: usize = 0;
static mut PREV_START: u32 = 0;
static mut HAS_PREV: bool = false;

pub fn record(t_start: u32, t_release: u32) {
    unsafe {
        if N >= CAP {
            return;
        }
        let dispatch = t_start.wrapping_sub(t_release);
        // 理想释放点是 `origin + k * PERIOD`，不是 `k * PERIOD` ——
        // CYCCNT 清零与 SysTick 使能之间有几百周期的间隔，见 sched::ORIGIN。
        let phase = t_start.wrapping_sub(sched::origin()) % time::CYCLES_PER_MS;
        let period = if HAS_PREV {
            t_start.wrapping_sub(PREV_START)
        } else {
            0
        };

        DISPATCH[N] = dispatch;
        PHASE[N] = phase;
        PERIOD[N] = period;

        HAS_PREV = true;
        PREV_START = t_start;
        N += 1;
    }
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

/// 实验 02：按屏蔽时长分档输出。
///
/// 列：`stage,mask_cycles,idx,phase_cycles,dispatch_cycles`
pub fn dump_sweep() -> ! {
    stop_tick();

    let n = unsafe { N };
    rprintln!(
        "# exp02 interrupt-masking sweep: samples={} stages={} per_stage={}",
        n, STAGES, PER_STAGE
    );
    rprintln!("stage,mask_cycles,idx,phase_cycles,dispatch_cycles");

    unsafe {
        for i in 0..n {
            let st = i / PER_STAGE;
            rprintln!(
                "{},{},{},{},{}",
                st,
                load::MASKS[st],
                i % PER_STAGE,
                PHASE[i],
                DISPATCH[i]
            );
        }
    }

    rprintln!(
        "# overruns={} tick_missed={}",
        sched::OVERRUN.load(core::sync::atomic::Ordering::Relaxed),
        sched::TICK_MISSED.load(core::sync::atomic::Ordering::Relaxed)
    );
    rprintln!("# END");
    halt()
}

/// 实验 01：单档分布输出。
///
/// 列：`idx,latency_cycles,period_cycles`（`latency` 就是 `dispatch`）
pub fn dump_single() -> ! {
    stop_tick();

    let n = unsafe { N };
    rprintln!("# exp01 samples n={}", n);
    rprintln!("idx,latency_cycles,period_cycles");

    let mut min = u32::MAX;
    let mut max = 0u32;
    let mut sum: u64 = 0;

    unsafe {
        for i in 0..n {
            let l = DISPATCH[i];
            if l < min {
                min = l;
            }
            if l > max {
                max = l;
            }
            sum += l as u64;
            rprintln!("{},{},{}", i, l, PERIOD[i]);
        }
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
    halt()
}

/// 停 SysTick：否则中断会打断输出，而且此时已经采满，继续释放只会污染数据。
fn stop_tick() {
    unsafe {
        (0xE000_E010 as *mut u32).write_volatile(0);
    }
}

/// 用 bkpt 结束，作为"我跑完了"的信号。probe-rs 会因此以非零码退出，属正常。
fn halt() -> ! {
    loop {
        cortex_m::asm::bkpt();
    }
}
