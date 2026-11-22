//! 系统时钟：HSE 25MHz → PLL1。
//!
//! 参数取自板厂参考例程（`【1】参考例程/1.LED闪烁/Core/Src/main.c` 的
//! `SystemClock_Config`），不是我推的：HSE 25MHz / M=10 / N=220 / P=1 = 550MHz。
//!
//! # 两个极易写错的寄存器细节（已由读回值验证）
//!
//! 1. **`PLLCKSELR.DIVM1` 存原值**（M=10 就写 10），不是减一。
//! 2. **`PLL1DIVR` 的 N/P/Q/R 存减一**：`N1=219`、`P1=0`、`Q1=1`、`R1=1`。
//!
//! 依据：`stm32h7xx_hal_rcc.c:862-866` 的比较逻辑，以及 `:1438` 的
//! `pllvco = … * (N1 + 1)`。实测读回 `DIVR=0x010100db` 与预期完全一致。
//!
//! # 一个凭记忆写错过的寄存器
//!
//! 早期版本在 `PWR_CR3` 里等 `ACTVOSRDY`（bit13）。但 **H723 的 `PWR_CR3`
//! 没有这个位** —— 只有 BYPASS / LDOEN / SCUEN / VBE / VBRS / USB33DEN。
//! 那是个空转 200 万次的无效等待。
//!
//! 电压等级是否就绪要看 **`PWR_D3CR`**：`VOS[15:14]` + `VOSRDY[13]`。
//! 实测写 VOS0 后 `PWR_D3CR` 从 `0x6000` 变成 `0xE000`（VOS=0b11、VOSRDY=1），
//! 说明 VOS0 确实生效 —— 所以"升压没到位"这个假设被排除了。
//!
//! # 为什么每一步都带超时、都写进度标记
//!
//! 时钟配错最典型的表现是"板子毫无反应" —— 卡在等某个标志的死循环里，
//! 连 RTT 都出不来。所以每个等待都有上界，超时就打印原因并退回 HSI。
//!
//! 更进一步：每一步都往 `.uninit` 写进度标记（见 `measure::mark`）。
//! 切换过程中调试口可能失联，那时只能靠"擦 flash → 读 RAM"取标记定位死点。

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
const PWR_CSR1: usize = PWR_BASE + 0x04;
const PWR_D3CR: usize = PWR_BASE + 0x18;

// ---- RCC_CR ----
const CR_HSEON: u32 = 1 << 16;
const CR_HSERDY: u32 = 1 << 17;
const CR_PLL1ON: u32 = 1 << 24;
const CR_PLL1RDY: u32 = 1 << 25;

// ---- PWR ----
const CR3_LDOEN: u32 = 1 << 1;
const CR3_BYPASS: u32 = 1 << 0;
const D3CR_VOS_MASK: u32 = 0b11 << 14;
const D3CR_VOSRDY: u32 = 1 << 13;

/// VOS 档位编码。**这里是本文件最容易搞错的一处。**
///
/// `stm32h7xx_hal_pwr_ex.h` 里 `PWR_REGULATOR_VOLTAGE_SCALE0` 有**两套定义**，
/// 取决于芯片用 D3CR 还是 SRDCR 管电压：
///
/// ```c
/// // SRDCR 系列（部分 H7A/H7B）
/// SCALE0 = 0b11   SCALE1 = 0b10   SCALE2 = 0b01   SCALE3 = 0b00
/// // D3CR 系列（H723 属于这类）
/// SCALE0 = 0b00   SCALE1 = 0b11   SCALE2 = 0b10   SCALE3 = 0b01
/// ```
///
/// 早先按第一套写 `0b11`，以为是最高的 SCALE0，实际写成了 **SCALE1**。
/// 后果：200MHz 能跑（复位默认 SCALE3 的上限就在这附近），
/// 400/550 全部在切 SYSCLK 时挂死 —— 因为电压等级根本不够。
///
/// 实测读数：复位 `D3CR=0x6000`（SCALE3）；写 `0xE000` 得到 SCALE1；
/// 写 `0x0000` 才是真正的 **SCALE0**（`D3CR=CSR1=0x2000`，VOS=0b00）。
const VOS_SCALE0: u32 = 0b00 << 14;
const VOS_SCALE1: u32 = 0b11 << 14;

// ---- Flash ----
const ACR_LATENCY_MASK: u32 = 0xF << 0;
const ACR_WRHIGHFREQ_MASK: u32 = 0b11 << 4;

// ---- PLL ----
const PLLSRC_HSE: u32 = 2; // 0=HSI 1=CSI 2=HSE
const PLLM: u32 = 10;
const PLLCFGR_RGE_2_4MHZ: u32 = 1 << 2;
const PLLCFGR_VCOSEL_WIDE: u32 = 0 << 1;

// ---- CFGR ----
const CFGR_SW_MASK: u32 = 0b111;
const CFGR_SWS_MASK: u32 = 0b111 << 3;
const SW_PLL1: u32 = 3;
const SWS_PLL1: u32 = 3;

/// 等待循环的上界。64MHz 下约几十毫秒，远超任何正常锁定时间。
const TIMEOUT: u32 = 2_000_000;

/// 目标配置。
///
/// 分两档不是为了功能，是为了**隔离**：550MHz 那次切换会把板子搞死，
/// 在分不清"切换代码写错"还是"550 特有约束"之前，先用一个宽松的中间点
/// 把代码路径本身验证掉。
#[derive(Clone, Copy, PartialEq)]
pub enum Target {
    /// 200MHz：不需要 VOS0，AXI 200MHz 只需 2 个等待周期。
    /// 这是"切换机制是否正确"的判据，不是性能目标。
    M200,
    /// 400MHz：带 VOS0，但频率低于 550。
    ///
    /// 单变量隔离用：M200 已经证明切换代码本身正确，那么 550 的失败
    /// 要么来自 VOS0，要么来自频率本身。400 带 VOS0 →
    /// 通过则 VOS0 无罪，失败则 VOS0 是嫌疑。
    M400,
    /// 400MHz，**不改 VOS**（依赖复位默认 VOS1）。
    ///
    /// 与 `M400` 的唯一差别就是那一行 VOS0 写入。
    /// 通过 ⇒ VOS0 是罪魁；失败 ⇒ 问题在"频率超过 200"本身。
    M400NoVos,
    /// 200MHz CPU，但 **AXI 不分频**（AXI = 200MHz）。
    ///
    /// 与 `M200` 的唯一差别是 HPRE。用来把"AXI/flash 时钟"从"CPU 时钟"
    /// 里单独拎出来 —— 不用动链接脚本就能判断取指路径是否可疑。
    M200Axi200,
    /// 550MHz：VOS0 + 3 等待周期 + AXI 分频到 275MHz。
    M550,
}

/// 诊断阶段。
#[derive(Clone, Copy, PartialEq)]
pub enum Stage {
    /// 只配 PLL 并等锁定，**不切换系统时钟**。
    Lock,
    /// 完整切换。
    Full,
}

/// 实际运行频率。默认按复位值 64MHz，成功切到 PLL 后改为目标值。
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

/// 写 `CFGR.SW=PLL1` 并等 `SWS` 真正变成 PLL1。
///
/// 目前仍在 flash 上执行 —— 这是已知瓶颈，见第 8 步的注释。
fn switch_and_wait() -> bool {
    modify(RCC_CFGR, CFGR_SW_MASK, SW_PLL1);
    for _ in 0..TIMEOUT {
        if rd(RCC_CFGR) & CFGR_SWS_MASK == SWS_PLL1 << 3 {
            return true;
        }
    }
    false
}

/// 有界等待某个位。返回 false 表示超时。
fn wait_set(addr: usize, mask: u32, want: bool) -> bool {
    for _ in 0..TIMEOUT {
        if (rd(addr) & mask != 0) == want {
            return true;
        }
    }
    false
}

/// 配置并（取决于 `stage`）切换系统时钟。
///
/// 任何一步失败都保持/回退到 HSI 64MHz，并返回 Err(原因)。
pub fn init(target: Target, stage: Stage) -> Result<u32, &'static str> {
    use rtt_target::rprintln;

    // 目标参数：PLL 倍频、是否要 VOS0、Flash 等待周期、WRHIGHFREQ、HPRE、目标频率
    let (plln, vos0, latency, wrhf, hpre, target_hz) = match target {
        // 200MHz：25/10*80/1。VOS 保持复位默认（VOS1 够用），AXI 200MHz 要 2 WS
        Target::M200 => (80u32, false, 2u32, 1u32, 1u32, 200_000_000u32),
        // 550MHz：25/10*220/1。VOS0 必须；AXI 275MHz 要 3 WS
        // 400MHz：25/10*160/1。AXI 200MHz 要 2 WS
        Target::M400 => (160u32, true, 2u32, 1u32, 1u32, 400_000_000u32),
        // 注：400MHz 在 SCALE1 的范围内，550 才需要 SCALE0
        Target::M400NoVos => (160u32, false, 2u32, 1u32, 1u32, 400_000_000u32),
        Target::M200Axi200 => (80u32, false, 2u32, 1u32, 0u32, 200_000_000u32),
        Target::M550 => (220u32, true, 3u32, 2u32, 1u32, 550_000_000u32),
    };

    // 1) 供电：确认 LDO 模式。
    //    这里**不能**等 ACTVOSRDY —— H723 的 PWR_CR3 没有这个位。
    crate::measure::mark(1, 0xC0DE_0001);
    rprintln!("# clk[1] PWR_CR3={:#010x}", rd(PWR_CR3));
    modify(PWR_CR3, CR3_LDOEN | CR3_BYPASS, CR3_LDOEN);
    rprintln!("# clk[1] PWR_CR3={:#010x} (LDO mode)", rd(PWR_CR3));

    // 2) 电压等级。只有 550MHz 需要 VOS0；先升压再升频，顺序不能反。
    crate::measure::mark(1, 0xC0DE_0002);
    if vos0 {
        rprintln!("# clk[2] PWR_D3CR={:#010x}", rd(PWR_D3CR));
        // 一律用 SCALE0（编码 0b00）。它是最高档，对任何目标频率都安全；
        // 而 SCALE1 在 H72x/73x 上的上限可能只有 275MHz，用它跑 400 会失败。
        modify(PWR_D3CR, D3CR_VOS_MASK, VOS_SCALE0);
        if !wait_set(PWR_D3CR, D3CR_VOSRDY, true) {
            return Err("VOS0 未就绪");
        }
        // CSR1 的 ACTVOS[15:14] 才是"当前实际生效的电压等级"，
        // D3CR 的 VOSRDY 只说明写入被接受。两者都要看。
        rprintln!(
            "# clk[2] VOS written, D3CR={:#010x} CSR1={:#010x} (ACTVOS bits 15:14)",
            rd(PWR_D3CR),
            rd(PWR_CSR1)
        );
    } else {
        rprintln!("# clk[2] VOS left at reset default");
    }
    crate::measure::mark(1, 0xC0DE_0003);

    // 3) 开 HSE 并等就绪
    modify(RCC_CR, CR_HSEON, CR_HSEON);
    if !wait_set(RCC_CR, CR_HSERDY, true) {
        return Err("HSE 未起振（晶振有问题？）");
    }
    crate::measure::mark(1, 0xC0DE_0004);
    rprintln!("# clk[3] HSE ready, RCC_CR={:#010x}", rd(RCC_CR));

    // 4) 配 PLL1
    modify(RCC_PLLCKSELR, 0b11, PLLSRC_HSE);
    modify(RCC_PLLCKSELR, 0x3F << 4, PLLM << 4); // DIVM1 = 原值
    modify(
        RCC_PLLCFGR,
        (0b11 << 2) | (1 << 1) | (1 << 0),
        PLLCFGR_RGE_2_4MHZ | PLLCFGR_VCOSEL_WIDE,
    );
    // N/P/Q/R 全部存减一
    wr(
        RCC_PLL1DIVR,
        (plln - 1) | (0 << 9) | (1 << 16) | (1 << 24),
    );

    // 5) 启动 PLL 并等锁定
    modify(RCC_CR, CR_PLL1ON, CR_PLL1ON);
    if !wait_set(RCC_CR, CR_PLL1RDY, true) {
        return Err("PLL1 未锁定（参数或供电）");
    }
    crate::measure::mark(2, 0xC0DE_0005);
    rprintln!(
        "# clk[5] PLL locked: CR={:#010x} DIVR={:#010x}",
        rd(RCC_CR),
        rd(RCC_PLL1DIVR)
    );

    if stage == Stage::Lock {
        rprintln!("# clk: Lock-only stage, staying on HSI");
        return Ok(cpu_hz());
    }

    // 6) 升频前先加 Flash 等待周期 —— 顺序反了会取指失败
    modify(
        FLASH_ACR,
        ACR_LATENCY_MASK | ACR_WRHIGHFREQ_MASK,
        latency | (wrhf << 4),
    );
    dsb_isb();
    crate::measure::mark(1, 0xC0DE_0006);
    rprintln!("# clk[6] FLASH_ACR={:#010x}", rd(FLASH_ACR));

    // 7) 设总线分频。H7 的分频字段是分级编码：0=/1, 1=/2, 2=/4 …
    modify(RCC_D1CFGR, 0xF << 8, 0 << 8); // D1CPRE = /1
    modify(RCC_D1CFGR, 0xF << 0, hpre << 0); // HPRE
    modify(RCC_D1CFGR, 0b111 << 4, 1 << 4); // D1PPRE = /2 → APB3
    modify(RCC_D2CFGR, 0b111 << 4, 1 << 4); // D2PPRE1 = /2 → APB1
    modify(RCC_D2CFGR, 0b111 << 8, 1 << 8); // D2PPRE2 = /2 → APB2
    modify(RCC_D3CFGR, 0b111 << 4, 1 << 4); // D3PPRE = /2 → APB4
    crate::measure::mark(2, 0xC0DE_0007);
    rprintln!(
        "# clk[7] dividers: D1CFGR={:#010x} D2CFGR={:#010x} D3CFGR={:#010x}",
        rd(RCC_D1CFGR),
        rd(RCC_D2CFGR),
        rd(RCC_D3CFGR)
    );

    // 8) 切系统时钟到 PLL1，并等切换真的生效。
    //
    // 已知瓶颈：这一步在 flash 上执行，而 SYSCLK 会在这一拍从 64MHz 跳到目标频率。
    // 核 halt 时由主机侧写同样的寄存器能切成功（实测 CFGR=0x1b @550MHz），
    // 但固件执行时 400/550 都会失败 —— 差别只在"CPU 是否在取指"。
    // 原计划把这几行搬进 RAM 执行，但本工具链下 `link_section` 没能把函数放进 `.data`
    // （.data 始终只有 4 字节），所以该尝试未生效，见文档。
    if !switch_and_wait() {
        modify(RCC_CFGR, CFGR_SW_MASK, 0);
        return Err("切换 SYSCLK 到 PLL1 超时");
    }
    crate::measure::mark(3, 0xC0DE_0008);
    rprintln!("# clk[8] SYSCLK = PLL1, CFGR={:#010x}", rd(RCC_CFGR));

    CPU_HZ.store(target_hz, Ordering::Relaxed);
    Ok(target_hz)
}
