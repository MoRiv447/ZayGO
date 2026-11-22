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

/// 第一个 tick 落在 CYCCNT 周期的哪个位置。
///
/// 为什么要这个：`time::init()` 先清零 CYCCNT，之后才使能 SysTick，
/// 两者之间隔着几百个周期。所以 tick 边界在 CYCCNT 上**不对齐于 0**，
/// 而是对齐于某个固定偏移（实测约 600 周期）。
///
/// 后果：任何"等到周期末尾再动手"的逻辑，如果按 0 对齐就会整体偏掉这个偏移，
/// 导致小屏蔽时长完全盖不住释放点 —— 第一次跑就是这么错的。
static ORIGIN: AtomicU32 = AtomicU32::new(0);
static ORIGIN_SET: AtomicBool = AtomicBool::new(false);

/// 下一次 tick 的**期望**时刻（由上上次累加一个周期得到）。
static EXPECTED: AtomicU32 = AtomicU32::new(0);

/// 被硬件静默合并掉的 tick 数。
///
/// 为什么要单独统计：屏蔽超过一个周期时，SysTick 的 pending 位只会被置起一次，
/// 中间那些 tick 根本不会产生 ISR 入口。所以 `OVERRUN`（"上一轮还没跑完"）
/// 恒为 0 —— 它检测不到这种情况。必须拿实际时刻和期望时刻比。
pub static TICK_MISSED: AtomicU32 = AtomicU32::new(0);
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

    // SHPR3 (0xE000ED20) 的位段布局极易记反，这里写明：
    //   bits[23:16] = PendSV  (异常 14)
    //   bits[31:24] = SysTick (异常 15)
    //
    // 本项目要求：PendSV 最低（0xFF）、SysTick 最高（0x00）。
    //
    // 写反的后果不是"性能略差"，而是直接崩：
    //   yield_now() 在 SysTick 里置 PendSV pending → PendSV 优先级最高，
    //   立刻抢占 SysTick → 变成嵌套异常，入栈的是 Handler 模式现场（MSP），
    //   而 naked 处理器只按 PSP 保存 r4-r11 → 现场全错，
    //   最终 LR 变成 0xFFFFFFF1（返回 Handler 模式）→ bx lr → INVPC。
    //   指纹：保存的 r7 紧贴 MSP 顶（0x2404FFB8 一类地址）。
    //
    // 复位后所有异常优先级都是 0（最高），必须显式改，否则同样会抢占。
    unsafe {
        let shpr3 = 0xE000_ED20 as *mut u32;
        let mut v = shpr3.read_volatile();
        v &= !(0xFFu32 << 16);
        v |= 0xFF << 16; // PendSV  → 最低
        v &= !(0xFFu32 << 24);
        v |= 0x00 << 24; // SysTick → 最高
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

/// 读取任务保存的栈指针。仅供故障诊断使用。
pub fn sp_of(i: usize) -> u32 {
    unsafe { SP[i] }
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
    let t = time::now();
    // 只在第一个 tick 记一次：之后 tick 都落在同一相位上
    if !ORIGIN_SET.swap(true, Ordering::Relaxed) {
        ORIGIN.store(t % time::cycles_per_ms(), Ordering::Relaxed);
        EXPECTED.store(t, Ordering::Relaxed);
    } else {
        let exp = EXPECTED
            .load(Ordering::Relaxed)
            .wrapping_add(time::cycles_per_ms());
        let late = t.wrapping_sub(exp);
        // 守卫：`t < exp` 时 wrapping_sub 会把差变成约 2^32 的天文数字，
        // 除掉周期正好得到 67173 这种"看似合理"的假值。必须挡掉。
        // 上限取 4 个周期，足够覆盖最长的屏蔽档位（96000 = 1.5 周期）。
        let n = if late < time::cycles_per_ms() * 4 {
            late / time::cycles_per_ms()
        } else {
            0
        };
        if n > 0 {
            TICK_MISSED.fetch_add(n, Ordering::Relaxed);
            // **必须重新对齐**：早期版本漏了这一步，漂移会被后续每个 tick 反复累加，
            // 一个 128 样本的档位报出 4200 这种量级的假数字。
            // 对齐之后，每次只统计"这一拍到底吞了几个整周期"。
            EXPECTED.store(exp.wrapping_add(n * time::cycles_per_ms()), Ordering::Relaxed);
        } else {
            EXPECTED.store(exp, Ordering::Relaxed);
        }
    }
    RELEASE_TS.store(t, Ordering::Relaxed);
    RT_RELEASED.store(true, Ordering::Relaxed);
    yield_now();
}

/// 释放时刻（周期计数器值）。RT 任务用它算调度延迟。
pub fn release_ts() -> u32 {
    RELEASE_TS.load(Ordering::Relaxed)
}

/// tick 相位原点。所有"相对理想释放点"的计算都必须减掉它。
pub fn origin() -> u32 {
    ORIGIN.load(Ordering::Relaxed)
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
            measure::record(t_start, t_release);
        }
        load::rt_work();
        clear_release();

        if load::is_sweep() {
            // 屏蔽扫描：每档采满就换下一档，最后一档采完输出
            if measure::is_stage_full() && !load::advance_stage() {
                measure::dump_sweep();
            }
        } else if measure::len() >= measure::CAP {
            measure::dump_single();
        }
    }
}
