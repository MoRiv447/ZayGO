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

/// 复位后的系统时钟。
///
/// H723 复位默认走 HSI 64MHz，本实验**刻意不配 PLL**：
///   - PLL 配错会直接挂死且没有任何输出，调试成本极高
///   - 本实验测的是「有负载 vs 无负载」的**相对**差异，
///     64MHz 还是 550MHz 不改变结论方向
///
/// 若后续加了 PLL 配置，此常量必须同步修改，否则所有换算都是错的。
pub const CPU_HZ: u32 = 64_000_000;

/// 1ms 对应的周期数。
pub const CYCLES_PER_MS: u32 = CPU_HZ / 1000;

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
    cycles / (CPU_HZ / 1_000_000)
}
