//! 三个负载生成器 —— 实验的自变量。
//!
//! 为什么必须区分 `Spin` 和 `Web`：纯死循环是**最好情况** ——
//! 没有内存访问、没有分支预测失败、不占总线带宽。
//! 真实的 HTTP 解析 + 帧缓冲填充会触发 cache miss 和总线争用，
//! 干扰会明显更大。只测死循环会低估风险。

use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    /// L-A 基线：极轻空转（只有取指与分支，无内存访问）。
    Idle = 0,
    /// L-B 满载：纯算术死循环，不碰内存。
    Spin = 1,
    /// L-C 模拟 Web：分支密集的"解析" + 写内存的"渲染"。
    Web = 2,
}

static MODE: AtomicU8 = AtomicU8::new(0);

pub fn set_mode(m: Mode) {
    MODE.store(m as u8, Ordering::Relaxed);
}

fn mode() -> Mode {
    match MODE.load(Ordering::Relaxed) {
        1 => Mode::Spin,
        2 => Mode::Web,
        _ => Mode::Idle,
    }
}

/// 低优先级任务每轮调用一次。单轮要短（几千周期），
/// 这样抢占点足够密集，才能测到真实的响应延迟分布。
#[inline(never)]
pub fn load_iteration() {
    match mode() {
        Mode::Idle => {
            // 原本这里用 `wfi()` 表示"完全空闲"。实测这条路走不通：
            // 核心一进 Sleep，SWD 调试时钟就停，probe-rs 立刻报
            // "Target device responded with a FAULT response" 并失联，
            // 连 `probe-rs read` 都读不到，只能 connect-under-reset 救回来。
            //
            // 已试过置 DBGMCU_CR.DBG_SLEEP=1（见 main.rs 的 keep_debug_in_sleep），
            // 仍未解决 —— 具体原因待查，可能是 H7 的 D3 域调试时钟另有开关。
            //
            // 改用极轻空转，三个理由：
            // 1) 不产生内存访问、不占总线，对实时任务的干扰最小；
            // 2) 反而更纯粹 —— 排除了 WFI 唤醒延迟，测到的就是调度本身的开销；
            // 3) 三种模式因此都是"忙等"，对比是同口径的，不会把休眠/唤醒的
            //    差异混进"负载差异"里。
            //
            // 代价：这不是真正的"空闲系统"，功耗也不为零。作为调度延迟基线够用。
            for _ in 0..8 {
                cortex_m::asm::nop();
            }
            core::hint::black_box(());
        }
        Mode::Spin => spin_round(),
        Mode::Web => web_round(),
    }
}

/// 纯寄存器运算。刻意不加内存访问，用来分离"调度开销"与"内存/总线干扰"。
#[inline(never)]
fn spin_round() {
    let mut a = 0x1234_5678u32;
    let mut b = 0x9ABC_DEF0u32;
    for _ in 0..512 {
        a = a.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        b ^= a.rotate_left(7);
        a = a.wrapping_add(b >> 3);
    }
    // black_box 防止整个循环被优化掉 —— 否则测的是空循环
    core::hint::black_box((a, b));
}

static mut BUF: [u8; 256] = [0; 256];
static mut FB: [u32; 128] = [0; 128];

/// 模拟管理面负载：读缓冲（分支密集）+ 写帧缓冲（占内存带宽）。
#[inline(never)]
fn web_round() {
    unsafe {
        let buf = &raw mut BUF as *mut u8;
        let mut tokens = 0u32;
        let mut sum = 0u32;
        for i in 0..256 {
            let b = buf.add(i).read();
            if b == b' ' || b == b'\n' || b == b',' {
                tokens += 1;
            }
            sum = sum.wrapping_add(b as u32);
        }

        let fb = &raw mut FB as *mut u32;
        for i in 0..128 {
            fb.add(i).write(sum ^ (i as u32).wrapping_mul(2_654_435_761));
        }

        core::hint::black_box((tokens, sum));
    }
}

static RT_DATA: [u32; 32] = [0x1111_1111; 32];

/// 实时任务的固定工作量。保持很小：本实验关心的是"什么时候轮到它跑"，
/// 不是"它跑多久"。若这里太重，会引入超期，混淆两个不同的指标。
#[inline(never)]
pub fn rt_work() {
    let mut acc = 0u32;
    for v in RT_DATA.iter() {
        acc = acc.wrapping_add(*v);
    }
    core::hint::black_box(acc);
}
