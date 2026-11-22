//! USART1 轮询驱动（PA9 = TX, PA10 = RX）。
//!
//! 板子把 USART1 和 SWD 放在同一个 2×4 排针上（见 `docs/HARDWARE-NOTES.md` 第 0.1 节），
//! 所以接串口线时 SWD 仍然可用。
//!
//! # 为什么是轮询而不是中断/DMA
//!
//! 这个串口跑在**管理面**，它的职责是"可以被随时打断"，而不是"低延迟"。
//! 轮询实现最简单、不会和实时面抢中断优先级，也便于把整条路径放在
//! LOAD 任务里跑 —— 正好验证"慢速控制台不干扰实时任务"这个论点。
//!
//! 需要吞吐时再换 DMA，但那是 P2 的事。

use core::fmt;

const USART1: usize = 0x4001_1000;
const GPIOA: usize = 0x5802_0000;
const RCC_AHB4ENR: usize = 0x5802_44E0;
const RCC_APB2ENR: usize = 0x5802_44F0;

// USART 寄存器偏移
const CR1: usize = 0x00;
const BRR: usize = 0x0C;
const ISR: usize = 0x1C;
const RDR: usize = 0x24;
const TDR: usize = 0x28;

const CR1_UE: u32 = 1 << 0;
const CR1_RE: u32 = 1 << 2;
const CR1_TE: u32 = 1 << 3;
const ISR_RXNE: u32 = 1 << 5;
const ISR_TXE: u32 = 1 << 7;

// GPIO 寄存器偏移
const MODER: usize = 0x00;
const OSPEEDR: usize = 0x08;
const PUPDR: usize = 0x0C;
const AFRH: usize = 0x24;

#[inline(always)]
fn rd(addr: usize) -> u32 {
    unsafe { (addr as *const u32).read_volatile() }
}

#[inline(always)]
fn wr(addr: usize, v: u32) {
    unsafe { (addr as *mut u32).write_volatile(v) }
}

#[inline(always)]
fn modify(addr: usize, mask: u32, v: u32) {
    wr(addr, (rd(addr) & !mask) | (v & mask));
}

/// 初始化 USART1。`fck` 是 APB2 时钟频率。
///
/// 当前固件跑 HSI 64MHz、默认分频全为 /1，所以 APB2 = 64MHz。
/// 若将来改了 PLL 或 APB2 分频，**必须把真实值传进来**，否则波特率是错的
/// 且不会有任何报错 —— 只会看到乱码。
pub fn init(fck: u32, baud: u32) {
    unsafe {
        // 1) 开时钟
        modify(RCC_AHB4ENR, 1 << 0, 1 << 0); // GPIOAEN
        modify(RCC_APB2ENR, 1 << 4, 1 << 4); // USART1EN
        let _ = rd(RCC_APB2ENR); // 读一次，确保写生效（外设时钟门控的常见要求）

        // 2) PA9/PA10 -> 复用功能 7 (USART1)
        //    MODER 每引脚 2 位：10 = 复用
        modify(GPIOA + MODER, (0b11 << 18) | (0b11 << 20), (0b10 << 18) | (0b10 << 20));
        modify(GPIOA + OSPEEDR, (0b11 << 18) | (0b11 << 20), (0b11 << 18) | (0b11 << 20));
        modify(GPIOA + PUPDR, (0b11 << 18) | (0b11 << 20), (0b01 << 18)); // RX 上拉
        // AFRH 每引脚 4 位：PA9 -> [7:4]，PA10 -> [11:8]
        modify(GPIOA + AFRH, (0xF << 4) | (0xF << 8), (0x7 << 4) | (0x7 << 8));

        // 3) 波特率。OVER8=0 时 USARTDIV = fck / (16 * baud)，
        //    BRR 里整数部分在 [15:4]、小数部分（1/16 为单位）在 [3:0]。
        //    用 u64 做定点运算，避免 u32 相乘溢出（64e6 * 100 已经超 u32）。
        let scaled = (fck as u64 * 100) / (16u64 * baud as u64);
        let mant = (scaled / 100) as u32;
        let frac = ((scaled % 100) * 16 / 100) as u32;
        wr(USART1 + BRR, (mant << 4) | (frac & 0xF));

        // 4) 使能收发
        wr(USART1 + CR1, CR1_UE | CR1_TE | CR1_RE);
    }
}

/// 不阻塞地取一个字节。
pub fn try_read() -> Option<u8> {
    if rd(USART1 + ISR) & ISR_RXNE != 0 {
        Some(rd(USART1 + RDR) as u8)
    } else {
        None
    }
}

/// 发一个字节（发送寄存器空就返回，不等移完）。
pub fn write_byte(b: u8) {
    while rd(USART1 + ISR) & ISR_TXE == 0 {}
    wr(USART1 + TDR, b as u32);
}

/// 把 `core::fmt::Write` 接到 USART1 上。
pub struct Writer;

impl fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            // 统一把 \n 变成 \r\n：串口终端需要回车，否则光标只下移不回到行首
            if b == b'\n' {
                write_byte(b'\r');
            }
            write_byte(b);
        }
        Ok(())
    }
}
