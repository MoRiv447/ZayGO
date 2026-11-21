//! 实验 01：抢占式调度器的干扰量化
//!
//! 目标：在写完整内核之前，测出「低优先级负载对高优先级任务的调度延迟影响」，
//! 以及「带宽限制能否把干扰压下去」。
//!
//! 运行方式见 README。改动 `MODE` 即可切换实验点，无需改其它代码。

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

mod context;
mod load;
mod measure;
mod sched;
mod time;

use cortex_m_rt::entry;
use rtt_target::{rprintln, rtt_init_print};

/// 当前实验点。改这一行即可，其它代码不用动。
///
/// | 值            | 对应设计文档的实验 |
/// |---------------|--------------------|
/// | `Mode::Idle`  | 实验 1 基线        |
/// | `Mode::Spin`  | 实验 2 满载 CPU    |
/// | `Mode::Web`   | 实验 3 模拟 Web    |
const MODE: load::Mode = load::Mode::Idle;

#[entry]
fn main() -> ! {
    rtt_init_print!();

    rprintln!("=== exp01: 抢占式调度器干扰量化 ===");
    rprintln!("# CPU {} Hz（复位默认 HSI，未配 PLL）", time::CPU_HZ);
    rprintln!("# 周期 1ms，样本上限 {}", measure::CAP);
    rprintln!("# 模式 {:?}", MODE as u8);

    time::init();

    // SysTick 必须在调度器初始化之后才能开：init 里设置的是中断优先级，
    // 顺序反了会让第一次 tick 落在一个尚未配置好 PendSV 优先级的系统上。
    sched::init();

    load::set_mode(MODE);

    let cp = cortex_m::Peripherals::take().unwrap();
    let mut syst = cp.SYST;
    syst.set_clock_source(cortex_m::peripheral::syst::SystClkSource::Core);
    syst.set_reload(time::CYCLES_PER_MS - 1);
    syst.clear_current();
    syst.enable_interrupt();
    syst.enable_counter();

    rprintln!("# 启动");
    sched::start();
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // 注：若在 rtt 初始化之前 panic，这里可能没有输出。
    // 那种情况下表现是"板子无反应"，需靠调试器 halt 看 PC。
    rprintln!("PANIC: {}", info);
    loop {
        cortex_m::asm::bkpt();
    }
}
