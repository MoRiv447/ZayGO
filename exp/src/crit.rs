//! 带预算申报的临界区。
//!
//! # 这个模块解决什么
//!
//! 实验 02 测出：最坏释放延迟与中断屏蔽时长 **1:1** 增长。
//! 所以"这个固件最坏会延迟多久"就等于"全固件最长的那个临界区"。
//!
//! 一个这样重要的数字，不能散落在代码注释里。它必须是：
//! 被写下来（`crit-sections.toml`）、被检查（build.rs）、被报告（构建产物）、
//! 被实测（本模块）。
//!
//! # 两层，各自的能力边界
//!
//! | 层 | 时机 | 检查的东西 | 抓不到什么 |
//! |---|---|---|---|
//! | `build.rs` | 构建期 | 申报值 vs 允许上限 | 申报 120、实际 1200 的实现 |
//! | 本模块 | 运行期 | 实测值 vs 申报值 | 没被执行到的代码路径 |
//!
//! 两层都不是 WCET 证明。累积误差、缓存、流水线、编译器优化都能让实测值
//! 偏离真实最坏情况。**这里是"预算监视"，不是"时序证明"。**
//!
//! # 为什么要在 `record()` 里真的用一次
//!
//! 声明了却从不执行的站点，运行期最大值为 0 —— 看起来"完美达标"，
//! 实际上什么也没验证。所以样本写入路径真的走这个包装，让站点有真实负载。

use core::sync::atomic::{AtomicU32, Ordering};

use crate::time;

// 由 build.rs 从 crit-sections.toml 生成
include!(concat!(env!("OUT_DIR"), "/crit_sites.rs"));

/// 每个站点观测到的最大屏蔽时长（周期）。
///
/// 用 `AtomicU32` 而不是普通 static mut：这个值会在任务与中断的交界处被更新，
/// 虽然本固件里写路径是单线程的，但读路径（dump）在别处。
pub static SITE_MAX: [AtomicU32; SITE_COUNT] = [const { AtomicU32::new(0) }; SITE_COUNT];

/// 每个站点超限的次数。
pub static SITE_VIOLATIONS: [AtomicU32; SITE_COUNT] = [const { AtomicU32::new(0) }; SITE_COUNT];

/// 在关中断的状态下执行 `f`，并记录这次屏蔽了多久。
///
/// # 测量口径（很容易搞错，踩过一次）
///
/// 时长在**关中断状态下**取，即 `cpsid` 之后、`cpsie` 之前。
///
/// 一开始写成在 `interrupt::free` 返回之后再取，结果同一段代码报出 214 周期，
/// 而正常只有 92 —— 因为退出临界区时若正好有 pending 中断补进来，
/// **整个 ISR 的耗时都被算进了临界区**。那不是临界区长，那是中断进来了。
///
/// 代价：测量值不含 `cpsid`/`cpsie` 与调用开销，略微偏乐观（约 30-50 周期）。
/// 所以申报值应当比实测值留出这段余量。
#[inline(always)]
pub fn mask<F: FnOnce()>(site: usize, f: F) {
    let mut elapsed = 0u32;
    cortex_m::interrupt::free(|_| {
        let t0 = time::now();
        f();
        elapsed = time::elapsed(t0);
    });

    SITE_MAX[site].fetch_max(elapsed, Ordering::Relaxed);
    if elapsed > SITE_BUDGETS[site] {
        SITE_VIOLATIONS[site].fetch_add(1, Ordering::Relaxed);
    }
}

/// 无副作用的站点数，供 dump 遍历。
pub fn site_count() -> usize {
    SITE_COUNT
}
