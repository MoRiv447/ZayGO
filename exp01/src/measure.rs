//! 采样与统计。
//!
//! 记录两个量：
//!   - `latency` = t_start − t_release，**核心指标**
//!   - `period`  = 相邻两次 t_start 之差，衡量周期抖动
//!
//! 为什么核心指标不是执行时间（t_end − t_start）：
//! 执行时间只说明"任务跑了多久"，完全看不出"它被拖了多久"。
//! 很多人第一次测实时性就测错在这里 —— 数字很漂亮，但系统其实已经超期了。

use rtt_target::rprintln;

/// 8192 条 × 2 个 u32 = 64KB。@1ms 周期约 8.2 秒。
pub const CAP: usize = 8192;

static mut LAT: [u32; CAP] = [0; CAP];
static mut PER: [u32; CAP] = [0; CAP];
static mut N: usize = 0;
static mut PREV_START: u32 = 0;
static mut HAS_PREV: bool = false;

pub fn record(latency: u32, t_start: u32) {
    unsafe {
        if N >= CAP {
            return;
        }
        LAT[N] = latency;
        PER[N] = if HAS_PREV {
            t_start.wrapping_sub(PREV_START)
        } else {
            0 // 第一条没有前值，分析时跳过
        };
        HAS_PREV = true;
        PREV_START = t_start;
        N += 1;
    }
}

#[allow(dead_code)]
pub fn len() -> usize {
    unsafe { N }
}

pub fn is_full() -> bool {
    unsafe { N >= CAP }
}

/// 停 SysTick，打印全部样本，然后停机。
///
/// 必须在打印前关掉 SysTick：否则中断会打断输出，
/// 而且此时已经采满，继续释放只会污染串口/RTT 时序。
pub fn dump_and_halt() -> ! {
    // SysTick CTRL @ 0xE000E010：写 0 关闭计数与中断
    unsafe {
        (0xE000_E010 as *mut u32).write_volatile(0);
    }

    let n = unsafe { N };
    rprintln!("# exp01 samples n={}", n);
    rprintln!("idx,latency_cycles,period_cycles");

    let mut min = u32::MAX;
    let mut max = 0u32;
    let mut sum: u64 = 0;

    unsafe {
        for i in 0..n {
            let l = LAT[i];
            let p = PER[i];
            if l < min {
                min = l;
            }
            if l > max {
                max = l;
            }
            sum += l as u64;
            rprintln!("{},{},{}", i, l, p);
        }
    }

    let mean = if n > 0 { (sum / n as u64) as u32 } else { 0 };
    rprintln!("# min={} max={} mean={} overruns={}", min, max, mean,
        crate::sched::OVERRUN.load(core::sync::atomic::Ordering::Relaxed));
    rprintln!("# END");

    loop {
        cortex_m::asm::bkpt();
    }
}
