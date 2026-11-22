//! 上下文切换。
//!
//! # 为什么必须用 naked 函数
//!
//! 普通 Rust 函数有编译器生成的序言/尾声（压栈、保存被调用者寄存器）。
//! 但在上下文切换里，**"压栈"这个动作本身就是被测量的对象** ——
//! 多压一个字、寄存器保存顺序错一位，任务恢复后就是错的。
//! 所以整个函数体只允许一条 `naked_asm!`，编译器不许插入任何指令。
//!
//! `#[unsafe(naked)]` + `naked_asm!` 在 Rust 1.88 稳定。
//!
//! # 任务栈布局（地址从低到高）
//!
//! ```text
//! 偏移   内容                    谁写的
//! +0    EXC_RETURN (0xFFFFFFFD)  stmdb r0!, {lr}
//! +4    r4                        stmdb r0!, {r4-r11}
//! +8    r5
//! ...
//! +32   r11
//! +36   r0                       硬件异常入栈（8 个字）
//! +40   r1
//! +44   r2
//! +48   r3
//! +52   r12
//! +56   lr
//! +60   pc     ← 任务入口
//! +64   xpsr   ← 0x01000000，Thumb 位
//! ```
//!
//! 保存的 SP 指向 +0。恢复时先弹 EXC_RETURN 到 lr、再弹 r4-r11，
//! 此时 PSP 指向 +36（硬件异常帧，已按 8 字节对齐），
//! 最后 `bx lr` 用 EXC_RETURN 触发硬件异常返回，由硬件弹掉剩下 8 个字并把
//! SPSEL 切到 PSP（EXC_RETURN 的 bit2 = 1）。
//!
//! 校验对齐：若栈顶 8 字节对齐，则 SP = top-68，+36 处 = top-32，
//! 仍是 8 字节对齐 —— ARM 要求异常帧 8 字节对齐，这个布局刚好满足，不需要补丁位。

use core::arch::naked_asm;

/// 返回 Thread 模式、使用 PSP、无 FP 上下文。
///
/// **0xFFFFFFFD 这个值意味着本系统不用 FPU。**
/// 一旦某个任务用了浮点指令，硬件会做 lazy stacking，栈帧多出 S0-S15 + FPSCR，
/// 必须改用 0xFFFFFFE9 并在栈上预留 FP 区，否则恢复出来的寄存器全是错的。
/// 本固件全程整数运算，就是为了绕开这个坑。
const EXC_RETURN_THREAD_PSP: u32 = 0xFFFF_FFFD;

/// xPSR 只置 Thumb 位（bit 24）。ARMv7-M 要求该位恒为 1，否则取指即 fault。
const XPSR_THUMB: u32 = 0x0100_0000;

/// PendSV 处理函数。
///
/// 用 `#[unsafe(no_mangle)]` 命名为 `PendSV` 来覆盖 cortex-m-rt 的弱定义。
/// PendSV 的优先级必须设成最低（见 `sched::init`），这样它只会在
/// 所有其它中断都处理完之后才执行 —— 上下文切换绝不会打断中断处理。
#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn PendSV() {
    naked_asm!(
        // 取当前任务的栈指针。线程模式用 PSP，所以这里读到的是任务栈。
        "mrs   r0, psp",
        // 保存被调用者保存寄存器（r4-r11）。
        // 调用者保存寄存器 r0-r3/r12/lr/pc/xpsr 已由硬件异常入栈，不重复保存。
        "stmdb r0!, {{r4-r11}}",
        // 把 EXC_RETURN 也存到任务栈上。必须存：下面 bl 会覆盖 lr。
        "stmdb r0!, {{lr}}",
        // 调度：入参 r0 = 当前任务保存的 SP，返回值 r0 = 下一个任务的 SP。
        "bl    {schedule}",
        // 先恢复 EXC_RETURN，再恢复 r4-r11（顺序与保存相反）。
        "ldmia r0!, {{lr}}",
        "ldmia r0!, {{r4-r11}}",
        // 切到新任务的栈。
        "msr   psp, r0",
        // 异常返回：硬件用 lr 里的 EXC_RETURN 决定回到 Thread/Handler、用 MSP/PSP。
        "bx    lr",
        schedule = sym crate::sched::schedule,
    );
}

/// 在一块栈顶之上构造出「刚被中断打断」的现场，返回应当保存的 SP。
///
/// # Safety
/// - `base` 必须指向 `len` 个 u32 的可写区域
/// - `base` 地址须 8 字节对齐（`Stack` 结构已用 `repr(align(8))` 保证）
/// - 该区域之后不能再被其它代码当作普通数组读写
pub unsafe fn prepare(base: *mut u32, len: usize, entry: extern "C" fn() -> !) -> u32 {
    debug_assert!(len >= 17);
    let top = base.add(len) as usize;
    debug_assert_eq!(top % 8, 0, "栈顶必须 8 字节对齐");

    // 17 个字 = 68 字节，正好是「9 个软件保存 + 8 个硬件帧」
    let sp = (top - 17 * 4) as *mut u32;

    // 先整体清零：未初始化的寄存器值进入任务后可能引发难以定位的故障
    for i in 0..17 {
        sp.add(i).write(0);
    }

    sp.add(0).write(EXC_RETURN_THREAD_PSP); // 异常返回描述符
    sp.add(15).write(entry as usize as u32); // pc：任务入口
    sp.add(16).write(XPSR_THUMB); // xpsr：Thumb 位

    sp as u32
}
