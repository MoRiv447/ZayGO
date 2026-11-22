//! 实验 01：抢占式调度器的干扰量化
//!
//! 目标：在写完整内核之前，测出「低优先级负载对高优先级任务的调度延迟影响」，
//! 以及「带宽限制能否把干扰压下去」。
//!
//! 运行方式见 README。改动 `MODE` 即可切换实验点，无需改其它代码。

#![no_std]
#![no_main]
#![allow(static_mut_refs)]

mod cache;
mod clock;
mod cli;
mod dev;
mod ctl;
mod uart;
mod context;
mod crit;
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
const MODE: load::Mode = load::Mode::Cli;

/// 是否切到 PLL 550MHz。实验 01/02/03 用 `false`（复位默认 HSI 64MHz），
/// 实验 04 用 `true`。
///
/// 做成常量而不是运行时开关，理由同缓存：频率必须在任何计时开始之前就定下来。
const USE_PLL: bool = false;

/// 诊断阶段。先用 `Lock` 确认 PLL 参数本身没问题，再改 `Full`。
const PLL_STAGE: clock::Stage = clock::Stage::Full;

/// 目标频率。
///
/// 先用 `M200` 把"切换机制本身是否正确"这一层验证掉，再上 `M550`。
/// 直接冲 550 会把板子搞死，而且分不清是代码写错还是 550 特有约束。
const PLL_TARGET: clock::Target = clock::Target::M550;

/// L1 缓存配置。实验 01/02 用 `Off`，实验 03 用 `I` 与 `ID` 做两档对照。
///
/// 用常量而不是运行时开关：缓存必须在任何计时开始之前就确定状态，
/// 中途切换会让同一批样本处于两种条件下，数据无法归因。
///
/// 拆成两档的目的：把"代码取指变快"和"数据访问变快"的贡献分开。
/// 只报一个"I+D 一共省了多少"，说不清是哪一半省下来的。
const CACHE_MODE: CacheMode = CacheMode::Off;

#[derive(Clone, Copy, PartialEq)]
enum CacheMode {
    /// 全关（复位默认）。
    Off,
    /// 只开 I-Cache。
    I,
    /// I-Cache + D-Cache。
    ID,
}

/// 让调试口在 WFI / Sleep 期间保持在线。
///
/// 实测教训：`DBGMCU_CR.DBG_SLEEP`（bit 0）复位默认是 0，
/// 核心一进 WFI 调试时钟就停，SWD 立刻失联 ——
/// probe-rs 报 "Target device responded with a FAULT response"，
/// 之后连 CPUID 都读不到，必须 `probe-rs reset --connect-under-reset` 才能救回来。
///
/// exp01 的 Idle 模式让负载任务长期 WFI，不置这一位就采不到任何数据。
///
/// DBGMCU 基址是 **0x5C001000**（H7 特有，不是 F4/F7 的 0xE0042000——
/// 后者在这块芯片上读出全 0），CR 在 +0x04。
unsafe fn keep_debug_in_sleep() {
    let cr = 0x5C00_1004 as *mut u32;
    cr.write_volatile(cr.read_volatile() | 1);
}

#[entry]
fn main() -> ! {
    // 默认模式是 NoBlockSkip —— 主机来不及搬就静默丢包（实测 8192 条只剩 ~5690 条）。
    // BlockIfFull 会阻塞等待主机，只影响采样结束后的输出阶段，不污染任何测量值。
    // 宏的形参是 `$mode:path`，所以必须写完整路径，不能只写 BlockIfFull。
    rtt_init_print!(rtt_target::ChannelMode::BlockIfFull, 4096);

    rprintln!("=== exp01: 抢占式调度器干扰量化 ===");
    rprintln!("# CPU {} Hz", time::cpu_hz());
    rprintln!("# 周期 1ms，样本上限 {}", measure::CAP);
    rprintln!("# 模式 {:?}", MODE as u8);
    rprintln!("# uninit block at {:#010x}", measure::block_addr());

    // 上一次运行若因高时钟无法用 RTT，它会把结果锁进 .uninit 并触发系统复位。
    // 现在芯片回到 HSI，调试口可用，把那份数据打出来就完事了。
    if measure::has_results() {
        rprintln!("# 发现 .uninit 里存有上次结果，改为输出而不重新测量");
        measure::dump_stored();
    }

    // 必须在任何 WFI 之前完成，否则调试口先失联就再也改不了了
    unsafe {
        keep_debug_in_sleep();
    }

    // 以下这行必须在切换之前输出 —— 切换之后任何打印都可能阻塞。
    rprintln!("# 即将配置时钟");

    // 时钟必须在 time::init 之前定下来：CYCCNT 清零的时刻要与最终频率一致，
    // 否则所有周期数与时间的换算都会偏。
    // 缓存必须在配时钟**之前**开。
    // 这不是风格问题：板厂参考例程的顺序就是 SCB_EnableICache/DCache 先、
    // SystemClock_Config 后。550MHz 下取指带宽吃紧，先开缓存再升频是它的前提。
    if CACHE_MODE != CacheMode::Off {
        cache::enable_icache();
    }
    if CACHE_MODE == CacheMode::ID {
        cache::enable_dcache();
    }
    rprintln!(
        "# cache: icache={} dcache={}",
        cache::icache_enabled(),
        cache::dcache_enabled()
    );
    if USE_PLL {
        match clock::init(PLL_TARGET, PLL_STAGE) {
            // **成功时不能打印**：此时可能已经跑在目标频率上，调试口若已失步，
            // rprintln!（BlockIfFull）就会永久阻塞。成功与否由 results 里的
            // cpu_hz 字段说明，不靠这行日志。
            Ok(_hz) => {}
            Err(e) => rprintln!("# clock: PLL 配置失败（{}），继续跑 {} Hz", e, clock::cpu_hz()),
        }
    } else {
        rprintln!("# clock: HSI (reset default) {} Hz", clock::cpu_hz());
    }

    time::init();

    // 串口控制台初始化。
    // fCK 传 APB2 时钟：当前 HSI 64MHz、D2PPRE2 分频为 /1（见 clock.rs），
    // 所以 APB2 = 64MHz。若将来改了时钟或分频，这里必须同步改 ——
    // 传错不会报错，只会看到乱码。
    if cli::CONSOLE == cli::Console::Uart {
        uart::init(time::cpu_hz(), 115_200);
    }


    // SysTick 必须在调度器初始化之后才能开：init 里设置的是中断优先级，
    // 顺序反了会让第一次 tick 落在一个尚未配置好 PendSV 优先级的系统上。
    sched::init();

    load::set_mode(MODE);
    // 真正要测了，先把上一轮可能残留的结果块清掉
    measure::reset();

    let cp = cortex_m::Peripherals::take().unwrap();
    let mut syst = cp.SYST;
    syst.set_clock_source(cortex_m::peripheral::syst::SystClkSource::Core);
    syst.set_reload(time::cycles_per_ms() - 1);
    syst.clear_current();
    syst.enable_interrupt();
    syst.enable_counter();

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

/// 故障取证。
///
/// 为什么值得专门写：HardFault 的默认行为是死循环，什么也不说。
/// 而 CFSR/MMFAR/BFAR + 异常帧能直接指认是哪种 fault、哪条指令、哪个地址。
/// 没有这些，只能靠反汇编猜。
///
/// `ExceptionFrame` 由 cortex-m-rt 自动取出：它已经是硬件压栈后的那 8 个字，
/// 不需要自己算 MSP/PSP 偏移。
#[cortex_m_rt::exception]
unsafe fn HardFault(ef: &cortex_m_rt::ExceptionFrame) -> ! {
    let cfsr = (0xE000_ED28 as *const u32).read_volatile();
    let hfsr = (0xE000_ED2C as *const u32).read_volatile();
    let mmfar = (0xE000_ED34 as *const u32).read_volatile();
    let bfar = (0xE000_ED38 as *const u32).read_volatile();

    rprintln!("=== HARDFAULT ===");
    rprintln!("# pc   {:#010x}", ef.pc());
    rprintln!("# lr   {:#010x}", ef.lr());
    rprintln!("# xpsr {:#010x}", ef.xpsr());
    rprintln!("# r0 {:#010x} r1 {:#010x} r2 {:#010x} r3 {:#010x}",
        ef.r0(), ef.r1(), ef.r2(), ef.r3());
    rprintln!("# r12 {:#010x}", ef.r12());
    rprintln!("# cfsr {:#010x} hfsr {:#010x}", cfsr, hfsr);
    rprintln!("# mmfar {:#010x} bfar {:#010x}", mmfar, bfar);
    rprintln!("# sp_load {:#010x} sp_rt {:#010x}",
        sched::sp_of(0), sched::sp_of(1));

    // 关掉 SysTick，避免后续 tick 继续制造状态变化，污染现场。
    // 然后死等 —— 刻意不用 bkpt：bkpt 会让核心 halt，probe-rs 收到 halt 就退出，
    // RTT 缓冲区里还没被搬走的输出会全部丢失。wfi 空转则让 probe-rs 继续在线收数据。
    unsafe {
        (0xE000_E010 as *mut u32).write_volatile(0);
    }
    loop {
        cortex_m::asm::wfi();
    }
}
