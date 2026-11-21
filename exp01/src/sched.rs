//! 双优先级抢占式调度器 —— 本实验的被测对象。
//!
//! 刻意保持极简：只有两个任务、固定优先级、无时间片、无阻塞原语。
//! 用现成的 FreeRTOS/Zephyr 测出来的数字是**那个内核的**干扰，
//! 换一个内核就不成立；只有自研的最小实现才能把干扰归因清楚。
//!
//! # 任务模型
//!
//! - `LOAD`（低优先级）：永不主动让出，制造持续 CPU 压力。这就是被测量的干扰源。
//! - `RT`（高优先级）：由 SysTick 周期性释放，只做固定工作量的活。
//!
//! 抢占路径：SysTick(最高) → 置 PendSV pending → PendSV(最低) 做真正的栈切换。
//! 这样上下文切换只在所有中断处理完之后发生，绝不会打断中断。

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::context;
use crate::{load, measure, time};

pub const IDX_LOAD: usize = 0;
pub const IDX_RT: usize = 1;

const NO_TASK: usize = usize::MAX;

/// 任务栈。`repr(align(8))` 不是可选项：
/// 异常帧要求 8 字节对齐，栈顶不对齐会触发 INVSTATE 或栈对齐错误。
#[repr(align(8))]
struct Stack<const N: usize>([u32; N]);

static mut STACK_LOAD: Stack<1024> = Stack([0; 1024]);
static mut STACK_RT: Stack<1024> = Stack([0; 1024]);
/// 第一次切换前的"引导上下文"暂存区。内容会被丢弃，仅需保证 PendSV
/// 往里压 r4-r11+lr 时有合法地址，否则第一次 mrs psp 会读到 0 并触发 BusFault。
static mut STACK_BOOT: Stack<256> = Stack([0; 256]);

static mut SP: [u32; 2] = [0; 2];
static mut CUR: usize = NO_TASK;

static RT_RELEASED: AtomicBool = AtomicBool::new(false);
static RELEASE_TS: AtomicU32 = AtomicU32::new(0);
pub static OVERRUN: AtomicU32 = AtomicU32::new(0);

pub fn init() {
    unsafe {
        SP[IDX_LOAD] = context::prepare(&raw mut STACK_LOAD as *mut u32, 1024, load_task);
        SP[IDX_RT] = context::prepare(&raw mut STACK_RT as *mut u32, 1024, rt_task);
    }

    // AIRCR：先把 PRIGROUP 归零，让全部优先级位都作为「抢占优先级」。
    //
    // 为什么需要这一步：PRIGROUP 决定优先级位里哪几位是抢占位、哪几位是子优先级。
    // 子优先级不同的两个异常**不能互相抢占**。复位默认 PRIGROUP=0（4 位全是抢占位），
    // 但实测这块板子上的既有固件把它改成了 3 —— 那样 SysTick 与 PendSV 的抢占关系
    // 就要看分组，可能变成「不同优先级却不抢占」。
    //
    // 写 AIRCR 必须带 VECTKEY=0x05FA，否则写入被硬件忽略（读回不变，很难察觉）。
    // 其余位（SYSRESETREQ 等）必须保持 0，否则会触发系统复位。
    unsafe {
        let aircr = 0xE000_ED0C as *mut u32;
        aircr.write_volatile((0x05FA << 16) | (0 << 8));
    }

    // SHPR3：bit16-23 = SysTick 优先级，bit24-31 = PendSV 优先级。
    // 复位后所有异常优先级都是 0（最高），**必须显式改**：
    //   - SysTick 保持 0，保证能抢占任务
    //   - PendSV 设为最低，保证切换不打断任何中断处理
    // 漏掉这一步的典型症状：切换偶尔在中断里发生，任务栈被踩。
    unsafe {
        let shpr3 = 0xE000_ED20 as *mut u32;
        let mut v = shpr3.read_volatile();
        v &= !(0xFFu32 << 16);
        v |= 0x00 << 16;
        v &= !(0xFFu32 << 24);
        v |= 0xFF << 24;
        shpr3.write_volatile(v);
    }
}

/// 启动调度：切到 PSP 然后把控制权交给 LOAD 任务。
///
/// 只切 PSP、不动 SPSEL —— 第一次 PendSV 会用 EXC_RETURN(0xFFFFFFFD) 触发
/// 硬件异常返回，那时硬件自动把 SPSEL 置 1。手动切 SPSEL 反而容易在
/// 编译器还依赖 MSP 的时候把栈搞坏。
pub fn start() -> ! {
    let scratch_top = unsafe {
        (&raw mut STACK_BOOT as *mut u32).add(256) as usize
    };
    unsafe {
        core::arch::asm!("msr psp, {0}", in(reg) scratch_top, options(nostack));
    }
    loop {
        yield_now();
    }
}

/// 主动让出 CPU。返回时表示「又被调度上来了」。
///
/// 这是任务恢复的唯一入口点，所以紧跟其后的第一条指令应当就是打时间戳，
/// 任何夹在中间的判断都会计入测量误差。
#[inline(always)]
pub fn yield_now() {
    // ICSR.PENDSVSET。写 1 置位；PendSV 优先级最低，会在当前中断返回后立即执行。
    unsafe {
        (0xE000_ED04 as *mut u32).write_volatile(1 << 28);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
    }
}

/// 由 PendSV 的 naked 汇编调用。
///
/// 入参 `cur_sp` 是当前任务保存的栈指针（已含 EXC_RETURN + r4-r11），
/// 返回值是下一个任务的栈指针。
///
/// 不能内联：汇编按符号调用它，内联后就找不到符号了。
#[inline(never)]
pub extern "C" fn schedule(cur_sp: u32) -> u32 {
    unsafe {
        // 首次切换时 CUR 是哨兵，没有有效的 TCB 可写，直接丢弃引导上下文
        if CUR != NO_TASK {
            SP[CUR] = cur_sp;
        }
        let next = if RT_RELEASED.load(Ordering::Relaxed) {
            IDX_RT
        } else {
            IDX_LOAD
        };
        CUR = next;
        SP[next]
    }
}

// ---------------------------------------------------------------------------
// SysTick：周期性释放 RT 任务
// ---------------------------------------------------------------------------

/// 覆盖 cortex-m-rt 的弱定义。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn SysTick() {
    on_tick();
}

fn on_tick() {
    // 上一轮还没跑完就又到了释放时刻 —— 这是「超期/过载」的直接证据，
    // 必须单独计数。合并释放会让延迟统计偏乐观，掩盖真实问题。
    if RT_RELEASED.load(Ordering::Relaxed) {
        OVERRUN.fetch_add(1, Ordering::Relaxed);
        return;
    }
    RELEASE_TS.store(time::now(), Ordering::Relaxed);
    RT_RELEASED.store(true, Ordering::Relaxed);
    yield_now();
}

/// 释放时刻（周期计数器值）。RT 任务用它算调度延迟。
pub fn release_ts() -> u32 {
    RELEASE_TS.load(Ordering::Relaxed)
}

pub fn clear_release() {
    RT_RELEASED.store(false, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// 任务
// ---------------------------------------------------------------------------

/// 低优先级负载任务：永不主动让出，靠被抢占来体现干扰。
extern "C" fn load_task() -> ! {
    loop {
        load::load_iteration();
    }
}

/// 高优先级实时任务。
///
/// 循环体刻意从 `yield_now()` 开始：恢复点紧贴时间戳读取，
/// 避免把循环判断、原子读等开销算进调度延迟里。
extern "C" fn rt_task() -> ! {
    loop {
        yield_now();
        let t_start = time::now();
        let t_release = release_ts();
        // 首次调度可能尚未经过一次释放，此时 t_release 为 0，相减是垃圾值。
        // 直接跳过，避免污染统计。
        if t_release != 0 {
            measure::record(t_start.wrapping_sub(t_release), t_start);
        }
        load::rt_work();
        clear_release();

        if measure::is_full() {
            measure::dump_and_halt();
        }
    }
}
