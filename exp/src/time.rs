//! 周期计数器（DWT CYCCNT）封装。
//!
//! 为什么不直接用 `cortex_m::peripheral::DWT`：那套封装要开 feature、
//! 且多一层方法调用。本文件的读操作会被内联成一条 `ldr`，
//! 这是测量路径上唯一能接受的代价。
//!
//! 寄存器地址见 ARMv7-M ARM：DEMCR 0xE000EDFC，DWT 0xE0001000。

const DEMCR: *mut u32 = 0xE000_EDFC as *mut u32;
const DWT_CTRL: *mut u32 = 0xE000_1000 as *mut u32;
const DWT_CYCCNT: *mut u32 = 0xE000_1004 as *mut u32;

const DEMCR_TRCENA: u32 = 1 << 24;
const DWT_CTRL_CYCCNTENA: u32 = 1 << 0;

/// 当前实际运行频率。
///
/// 从 `clock` 模块取，不是常量：实验 01/02/03 跑在复位默认的 64MHz HSI 上，
/// 实验 04 切到 PLL 的 550MHz。所有周期→时间的换算都必须走这个函数，
/// 否则换频之后每一个微秒数都是错的（而且错得很自然，不会有人发现）。
pub use crate::clock::cpu_hz;

/// 1ms 对应的周期数。SysTick 重装载值也用它。
#[inline(always)]
pub fn cycles_per_ms() -> u32 {
    cpu_hz() / 1000
}

pub fn init() {
    unsafe {
        // TRCENA 必须先在 DEMCR 中置位，否则 DWT 寄存器写入无效。
        // 这是最容易漏的一步：不置位时 CYCCNT 永远是 0，表现为「所有延迟都是 0」。
        DEMCR.write_volatile(DEMCR.read_volatile() | DEMCR_TRCENA);
        DWT_CYCCNT.write_volatile(0);
        DWT_CTRL.write_volatile(DWT_CTRL.read_volatile() | DWT_CTRL_CYCCNTENA);
    }
}

/// 读当前周期计数。32 位、约 67 秒 @64MHz 后回绕，用 `elapsed` 处理。
#[inline(always)]
pub fn now() -> u32 {
    unsafe { DWT_CYCCNT.read_volatile() }
}

/// 差值计算，自动处理 32 位回绕。
/// 后续实验（中断延迟、周期抖动）会用到，本实验暂未调用。
#[allow(dead_code)]
#[inline(always)]
pub fn elapsed(from: u32) -> u32 {
    now().wrapping_sub(from)
}

/// 周期数 → 微秒（整数运算，避免浮点：本固件全程不使用 FPU，
/// 因为一旦用 FP，异常栈帧会多出 S0-S15/FPSCR，EXC_RETURN 也要改成 0xFFFFFFE9，
/// 上下文切换的假设就全变了）。
#[allow(dead_code)]
pub fn cycles_to_us(cycles: u32) -> u32 {
    cycles / (cpu_hz() / 1_000_000)
}
