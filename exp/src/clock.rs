//! 系统时钟：HSE 25MHz → PLL1 → 550MHz。
//!
//! 参数直接取自板厂参考例程（`【1】参考例程/1.LED闪烁/Core/Src/main.c` 的
//! `SystemClock_Config`），不是我推的：
//!
//! ```text
//! HSE 25MHz / M(10) = 2.5MHz   →  PLLRGE = 2~4MHz 档
//! 2.5MHz × N(220)  = 550MHz    →  VCO 宽范围（192~836MHz）
//! 550MHz / P(1)    = 550MHz    →  CPU
//! AXI  = 550 / 2 = 275MHz      →  所以 Flash 等待周期按 275MHz 取 3 WS
//! APB1/2/3/4 = 275 / 2 = 137.5MHz
//! ```
//!
//! # 两个极易写错的寄存器细节
//!
//! 1. **`PLLCKSELR.DIVM1` 存的是原值**（M=10 就写 10），不是减一。
//! 2. **`PLL1DIVR` 的 N/P/Q/R 存的都是减一**：
//!    `N1 = 219`、`P1 = 0`、`Q1 = 1`、`R1 = 1`。
//!
//! 这两条是从 HAL 源码里核出来的（`stm32h7xx_hal_rcc.c:862-866` 的比较逻辑，
//! 以及 `:1438` 的 `pllvco = ... * (N1 + 1)`），不是凭记忆。
//!
//! # 为什么每一步都带超时
//!
//! 时钟配错最典型的表现是"板子毫无反应"——PLL 永远不锁，代码卡在等 `PLL1RDY`
//! 的死循环里，连 RTT 都出不来。所以这里每个等待都有上界，超时就打印原因并
//! **退回 64MHz HSI 继续跑**。宁可按错误频率出一份带说明的报告，也不要静默死掉。

use core::arch::asm;
use core::sync::atomic::{AtomicU32, Ordering};

const RCC_BASE: usize = 0x5802_4400;
const PWR_BASE: usize = 0x5802_4800;
const FLASH_ACR: usize = 0x5200_2000;

const RCC_CR: usize = RCC_BASE + 0x00;
const RCC_CFGR: usize = RCC_BASE + 0x10;
const RCC_D1CFGR: usize = RCC_BASE + 0x18;
const RCC_D2CFGR: usize = RCC_BASE + 0x1C;
const RCC_D3CFGR: usize = RCC_BASE + 0x20;
const RCC_PLLCKSELR: usize = RCC_BASE + 0x28;
const RCC_PLLCFGR: usize = RCC_BASE + 0x2C;
const RCC_PLL1DIVR: usize = RCC_BASE + 0x30;

const PWR_CR3: usize = PWR_BASE + 0x0C;
const PWR_D3CR: usize = PWR_BASE + 0x18;

// ---- RCC_CR ----
const CR_HSEON: u32 = 1 << 16;
const CR_HSERDY: u32 = 1 << 17;
const CR_PLL1ON: u32 = 1 << 24;
const CR_PLL1RDY: u32 = 1 << 25;

// ---- PWR ----
const CR3_LDOEN: u32 = 1 << 1;
const CR3_ACTVOSRDY: u32 = 1 << 13;
const D3CR_VOS_MASK: u32 = 0b11 << 14;
const D3CR_VOS_SCALE0: u32 = 0b11 << 14;
const D3CR_VOSRDY: u32 = 1 << 13;

// ---- Flash ----
const ACR_LATENCY_MASK: u32 = 0xF << 0;
const ACR_WRHIGHFREQ_MASK: u32 = 0b11 << 4;
const LATENCY_3WS: u32 = 3 << 0; // 275MHz @ VOS0
const WRHIGHFREQ_2: u32 = 2 << 4; // 185 < HCLK <= 285 MHz

// ---- PLL ----
const PLLSRC_HSE: u32 = 2; // 0=HSI 1=CSI 2=HSE
const PLLM: u32 = 10; // 原值
const PLLN: u32 = 220; // 存 219
const PLLP: u32 = 1; // 存 0
const PLLQ: u32 = 2; // 存 1
const PLLR: u32 = 2; // 存 1
const PLLCFGR_RGE_2_4MHZ: u32 = 1 << 2;
const PLLCFGR_VCOSEL_WIDE: u32 = 0 << 1;

// ---- CFGR ----
const CFGR_SW_MASK: u32 = 0b111;
const CFGR_SWS_MASK: u32 = 0b111 << 3;
const SW_PLL1: u32 = 3;
const SWS_PLL1: u32 = 3;

/// 等待循环的上界。64MHz 下约几十毫秒，远超任何正常锁定时间。
const TIMEOUT: u32 = 2_000_000;

/// 实际运行频率。默认按复位值 64MHz，成功切到 PLL 后改为 550MHz。
static CPU_HZ: AtomicU32 = AtomicU32::new(64_000_000);

pub fn cpu_hz() -> u32 {
    CPU_HZ.load(Ordering::Relaxed)
}

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

fn dsb_isb() {
    unsafe { asm!("dsb", "isb") };
}

/// 有界等待某个位置位。返回 false 表示超时。
fn wait_set(addr: usize, mask: u32, want: bool) -> bool {
    for _ in 0..TIMEOUT {
        let got = rd(addr) & mask != 0;
        if got == want {
            return true;
        }
    }
    false
}

/// 诊断阶段。
///
/// 分两段是因为第一次尝试直接切 550MHz 把调试口搞死了（`WAIT response`，
/// 只能靠 `probe-rs erase --connect-under-reset` 救回来）。
/// 在不知道是哪一步出问题时，一次只动一件事。
#[derive(Clone, Copy, PartialEq)]
pub enum Stage {
    /// 只配 PLL 并等锁定，**不切换系统时钟**。用于验证 PLL 参数本身。
    Lock,
    /// 完整切换。
    Full,
    /// 完整切换，但**不改 D3PPRE**（APB4 分频）。
    ///
    /// 用来验证一个怀疑：DBGMCU 在 D3 域，改 D3 的分频可能把 SWD 调试逻辑一起搞停。
    /// 代价是 APB4 会跑到 275MHz（规格上限 137.5MHz），仅用于诊断。
    FullNoD3Pre,
}

/// 配置到 550MHz。任何一步失败都保持/回退到 HSI 64MHz，并返回 Err(原因)。
///
/// 每一步都打印一行 —— 卡死时最后一行就是现场，比任何事后推断都准。
pub fn init(stage: Stage) -> Result<u32, &'static str> {
    use rtt_target::rprintln;
    // 1) 供电：确认 LDO 模式
    rprintln!("# clk[1] PWR_CR3={:#010x}", rd(PWR_CR3));
    modify(PWR_CR3, CR3_LDOEN, CR3_LDOEN);
    rprintln!("# clk[1] PWR_CR3={:#010x} (LDOEN set)", rd(PWR_CR3));
    wait_set(PWR_CR3, CR3_ACTVOSRDY, true);

    // 2) 电压等级 VOS0 —— 550MHz 必须，先升压再升频
    rprintln!("# clk[2] PWR_D3CR={:#010x}", rd(PWR_D3CR));
    modify(PWR_D3CR, D3CR_VOS_MASK, D3CR_VOS_SCALE0);
    if !wait_set(PWR_D3CR, D3CR_VOSRDY, true) {
        return Err("VOS0 未就绪");
    }
    rprintln!("# clk[2] VOS0 ready, PWR_D3CR={:#010x}", rd(PWR_D3CR));

    // 3) 开 HSE 并等就绪
    modify(RCC_CR, CR_HSEON, CR_HSEON);
    if !wait_set(RCC_CR, CR_HSERDY, true) {
        return Err("HSE 未起振（晶振有问题？）");
    }
    rprintln!("# clk[3] HSE ready, RCC_CR={:#010x}", rd(RCC_CR));

    // 4) 配 PLL1
    modify(RCC_PLLCKSELR, 0b11, PLLSRC_HSE); // PLLSRC
    modify(RCC_PLLCKSELR, 0x3F << 4, PLLM << 4); // DIVM1 = 原值
    modify(
        RCC_PLLCFGR,
        (0b11 << 2) | (1 << 1) | (1 << 0),
        PLLCFGR_RGE_2_4MHZ | PLLCFGR_VCOSEL_WIDE | 0, // FRACEN=0
    );
    wr(
        RCC_PLL1DIVR,
        (PLLN - 1) | ((PLLP - 1) << 9) | ((PLLQ - 1) << 16) | ((PLLR - 1) << 24),
    );

    // 5) 启动 PLL 并等锁定
    modify(RCC_CR, CR_PLL1ON, CR_PLL1ON);
    if !wait_set(RCC_CR, CR_PLL1RDY, true) {
        return Err("PLL1 未锁定（参数或供电）");
    }
    rprintln!(
        "# clk[5] PLL locked: CR={:#010x} DIVR={:#010x} CFGR={:#010x}",
        rd(RCC_CR),
        rd(RCC_PLL1DIVR),
        rd(RCC_CFGR)
    );

    if stage == Stage::Lock {
        rprintln!("# clk: Lock-only stage, staying on HSI");
        return Ok(cpu_hz());
    }

    // 6) 升频前先加 Flash 等待周期 —— 顺序反了会取指失败
    modify(FLASH_ACR, ACR_LATENCY_MASK | ACR_WRHIGHFREQ_MASK, LATENCY_3WS | WRHIGHFREQ_2);
    dsb_isb();

    rprintln!("# clk[6] FLASH_ACR={:#010x}", rd(FLASH_ACR));

    // 7) 设总线分频。H7 的分频字段是分级编码：0=/1, 1=/2, 2=/4 …
    modify(RCC_D1CFGR, 0xF << 8, 0 << 8); // D1CPRE = /1 → CPU 550MHz
    modify(RCC_D1CFGR, 0xF << 0, 1 << 0); // HPRE   = /2 → AXI 275MHz
    modify(RCC_D1CFGR, 0b111 << 4, 1 << 4); // D1PPRE = /2 → APB3
    modify(RCC_D2CFGR, 0b111 << 4, 1 << 4); // D2PPRE1 = /2 → APB1
    modify(RCC_D2CFGR, 0b111 << 8, 1 << 8); // D2PPRE2 = /2 → APB2
    if stage != Stage::FullNoD3Pre {
        modify(RCC_D3CFGR, 0b111 << 4, 1 << 4); // D3PPRE = /2 → APB4
    } else {
        rprintln!("# clk[7] D3PPRE left at /1 (diagnostic only, out of spec)");
    }

    rprintln!(
        "# clk[7] dividers set: D1CFGR={:#010x} D2CFGR={:#010x} D3CFGR={:#010x}",
        rd(RCC_D1CFGR),
        rd(RCC_D2CFGR),
        rd(RCC_D3CFGR)
    );

    // 8) 切系统时钟到 PLL1，并等切换真的生效
    modify(RCC_CFGR, CFGR_SW_MASK, SW_PLL1);
    if !wait_set(RCC_CFGR, CFGR_SWS_MASK, true) {
        // 切不过去就切回 HSI，至少还能跑
        modify(RCC_CFGR, CFGR_SW_MASK, 0);
        return Err("切换 SYSCLK 到 PLL1 超时");
    }
    debug_assert_eq!(rd(RCC_CFGR) & CFGR_SWS_MASK, SWS_PLL1 << 3);

    CPU_HZ.store(550_000_000, Ordering::Relaxed);
    Ok(550_000_000)
}
