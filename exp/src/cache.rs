//! L1 缓存控制。
//!
//! STM32H7 复位后 I-Cache 与 D-Cache 默认**都是关的**（见 DM00272913）。
//! 所以实验 01/02 测的都是"无 cache"的数字，实验 03 专门测开了之后会怎样。
//!
//! # 为什么 D-Cache 不能简单打开
//!
//! 本固件的输出走 RTT：把字符写进 RAM 里的环形缓冲，由 probe-rs 通过 SWD
//! **直接读那块 RAM**。
//!
//! D-Cache 一开，`rprintln!` 的写入会停在 cache 里（dirty line），
//! 而 probe-rs 读的是物理 RAM —— 读到的是旧数据。表现是"输出莫名其妙缺行或乱序"，
//! 而且不会报任何错。
//!
//! 所以策略是：**采样期开 cache，dump 前把 D-Cache 清干净再关掉**。
//! dump 不在测量路径上，跑慢一点无所谓；用这个代价换 RTT 的可见性。

use core::arch::asm;

/// SCR (System Control Block) 寄存器。地址见 ARMv7-M ARM。
const SCB_CCR: *mut u32 = 0xE000_ED14 as *mut u32;
const SCB_CCSIDR: *const u32 = 0xE000_ED80 as *const u32;
const SCB_ICIALLU: *mut u32 = 0xE000_EF50 as *mut u32;
const SCB_DCISW: *mut u32 = 0xE000_EF60 as *mut u32;
const SCB_DCCISW: *mut u32 = 0xE000_EF74 as *mut u32;

const CCR_DC: u32 = 1 << 16;
const CCR_IC: u32 = 1 << 17;

#[inline(always)]
fn dsb_isb() {
    unsafe {
        asm!("dsb", "isb");
    }
}

/// 打开指令缓存。
///
/// 顺序不能省：先 `DSB/ISB` 保证之前的存储都完成，再整条失效 I-Cache
/// （防止取到旧的预取指令），最后才置使能位。
pub fn enable_icache() {
    unsafe {
        dsb_isb();
        SCB_ICIALLU.write_volatile(0);
        dsb_isb();
        SCB_CCR.write_volatile(SCB_CCR.read_volatile() | CCR_IC);
        dsb_isb();
    }
}

/// 打开数据缓存。
///
/// 打开前必须先按 set/way 整体失效：复位状态下 cache 里没有有效行，理论上可以不失效，
/// 但只要有任何一次早期访问被 cache 命中过，就会拿到脏数据。这一步不能省。
pub fn enable_dcache() {
    unsafe {
        dsb_isb();
        invalidate_all();
        SCB_CCR.write_volatile(SCB_CCR.read_volatile() | CCR_DC);
        dsb_isb();
    }
}

/// 按 set/way 失效整个 D-Cache。
///
/// 参数从 `CCSIDR` 读，不写死 —— 不同型号的组数/路数不同，
/// 写死 16KB/4way 在换芯片时会静默失效（只清了前一半）。
unsafe fn invalidate_all() {
    let ccsidr = SCB_CCSIDR.read_volatile();
    let ways = ((ccsidr >> 3) & 0x3FF) + 1;
    // 行大小固定 32 字节，所以 set 字段从 bit5 开始
    let sets = ((ccsidr >> 13) & 0x7FFF) + 1;

    for way in 0..ways {
        for set in 0..sets {
            SCB_DCISW.write_volatile((way << 30) | (set << 5));
        }
    }
    dsb_isb();
}

/// 按 set/way 把整个 D-Cache 写回并失效。
///
/// 这就是 dump 之前必须做的那一步：把还脏在 cache 里的 RTT 数据推到 RAM，
/// 否则 probe-rs 读不到。
unsafe fn clean_invalidate_all() {
    let ccsidr = SCB_CCSIDR.read_volatile();
    let ways = ((ccsidr >> 3) & 0x3FF) + 1;
    let sets = ((ccsidr >> 13) & 0x7FFF) + 1;

    for way in 0..ways {
        for set in 0..sets {
            SCB_DCCISW.write_volatile((way << 30) | (set << 5));
        }
    }
    dsb_isb();
}

/// 让 RTT 输出变得可见：清干净并关掉 D-Cache。
///
/// 在 dump 之前调用一次。之后所有 `rprintln!` 直接写 RAM，probe-rs 读得到。
///
/// 注意：ARM 要求**先 clean 再 disable**，直接清使能位会留下未写回的脏行，
/// 那些数据就此丢失（在 M7 上是"行为不可预测"，不是"自动写回"）。
pub fn prepare_for_rtt() {
    unsafe {
        if SCB_CCR.read_volatile() & CCR_DC != 0 {
            clean_invalidate_all();
            SCB_CCR.write_volatile(SCB_CCR.read_volatile() & !CCR_DC);
            dsb_isb();
        }
    }
}

/// 当前 D-Cache 是否开着（供运行报告打印）。
pub fn dcache_enabled() -> bool {
    unsafe { SCB_CCR.read_volatile() & CCR_DC != 0 }
}

/// 当前 I-Cache 是否开着。
pub fn icache_enabled() -> bool {
    unsafe { SCB_CCR.read_volatile() & CCR_IC != 0 }
}
