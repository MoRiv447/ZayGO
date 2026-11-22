# 中断屏蔽预算：在构建期强制

> 中文版。 [English version](MASK-BUDGET.md)
>
> **状态：2026-11-22 已实现并上板验证。**
> 这是本项目"可预测性"那一列里，第一个**能跑的机制**而不是一次测量。

---

## 一、为什么要有它

实验 02 测出：最坏任务释放延迟与中断屏蔽时长 **1:1** 增长
（七档 99.4%~100.6%，见 [`EXPERIMENT-02.md`](EXPERIMENT-02.md)）。

于是"这个固件最坏会迟多久"这个问题，答案就是**它里面最长的那个临界区**。

这么重要的数字不能散落在代码注释里。它必须被写下来、被检查、被报告、被实测。

---

## 二、两层，各自诚实的能力边界

| 层 | 时机 | 检查 | 抓不到 |
|---|---|---|---|
| `build.rs` | 构建期 | 申报预算 vs 允许上限 | 申报 120、实际跑 1200 的临界区 |
| `src/crit.rs` | 运行期 | 实测时长 vs 申报预算 | 从来没被执行到的代码路径 |

**两层都不是 WCET 证明。** 缓存、流水线、编译器优化、累积误差都能让实测值偏离真实最坏情况。
这是**预算监视**，不是时序验证。说成后者，就是这个项目一直在批评别人的那种毛病。

那这两层到底买到了什么：

- 这个数字**只有一处声明**，不再散落
- 超预算的申报**直接让构建失败**，不可能被误合进来
- 申报偏低（写小了）会被**实测抓到**，而不是靠 review 抓到

---

## 三、清单文件

`exp/crit-sections.toml`：

```toml
[budget]
max_mask_cycles = 800     # 整个固件的允许上限
fail_on_violation = true  # 默认 true；只警告的检查几乎等于没检查

[[site]]
name = "sample_ring_push"
file = "src/measure.rs"
purpose = "写入一条样本的过程不能被中断看到半成品"
budget_cycles = 120

[[site]]
name = "sweep_mask_stimulus"
file = "src/load.rs"
purpose = "实验 02 的合成激励 —— 故意很大"
budget_cycles = 97000
exempt = true
```

`exempt = true` 跳过构建检查，但**仍然出现在两份报告里**，
所以豁免是看得见的，不会被悄悄忘掉。

解析器是手写的（不引入 TOML 依赖），但**未知键直接报错** ——
拼错一个键却被静默忽略，正是这个项目反复踩到的那种失败。

---

## 四、构建期检查

任何未豁免站点的申报值超过 `max_mask_cycles`，构建直接停：

```
中断屏蔽预算超限 —— 构建失败

  站点 `sweep_mask_stimulus` 申报 96200 周期，超过上限 800 周期（超出 95400）

为什么是硬失败而不是警告：实验 02 测得最坏释放延迟与屏蔽时长 1:1 增长，
一个超限的临界区会直接让实时任务错过截止时间。
要么缩短临界区，要么在 crit-sections.toml 里显式 exempt = true 并写明理由。
```

（实测方式：临时清掉 `exempt` 跑一次 `cargo build --release`。）

每次构建成功也会打印报告，CI 可归档：

```
[mask-budget] max_mask_cycles = 800
[mask-budget] longest declared non-exempt critical section = 120
[mask-budget] headroom = 680 cycles
[mask-budget] sites:
[mask-budget]   sample_ring_push             120 cycles  把一条样本写入环形缓冲…
[mask-budget]   sweep_mask_stimulus        97000 cycles  EXEMPT  实验 02 的合成激励…
```

---

## 五、运行期检查

`crit::mask(site, || { ... })` 在关中断状态下执行闭包，记录观测到的最大时长与超限次数。

这段代码值得逐行看，因为**测量窗口**我写错过两次：

| 版本 | 量的是什么 | 症状 |
|---|---|---|
| 计时放在 `interrupt::free` 外面 | 临界区 **加上**退出时补进来的 pending ISR | 本体 54 周期的代码报出 214 |
| 计时放在里面（当前实现） | 只有本体 | 54 周期，稳定 |

当前实现：

```rust
pub fn mask<F: FnOnce()>(site: usize, f: F) {
    let mut elapsed = 0u32;
    cortex_m::interrupt::free(|_| {
        let t0 = time::now();
        f();
        elapsed = time::elapsed(t0);   // 在关中断状态下取，pending ISR 撑不进来
    });

    SITE_MAX[site].fetch_max(elapsed, Ordering::Relaxed);
    if elapsed > SITE_BUDGETS[site] {
        SITE_VIOLATIONS[site].fetch_add(1, Ordering::Relaxed);
    }
}
```

真实运行的输出：

```
# mask budget: declared_max=800 cycles
# site,budget,observed_max,violations
# sample_ring_push,120,54,0
# sweep_mask_stimulus,97000,96043,0
```

---

## 六、它实际抓到了什么

**我自己的两次错误申报**，就在第一个小时里。

| 站点 | 申报 | 实测 | 结果 |
|---|---|---|---|
| `sweep_mask_stimulus` | 96200（按 `PRE + M` 推导） | 96912 | 125 次超限 —— 推导漏了约 700 周期的包装与轮询开销 |
| `sweep_mask_stimulus` | 97000（按实测填） | 96043 | 0 次超限 |

申报值必须取自实测，不能取自推导。这就是运行期这一层存在的全部理由：
**推导只是假说，而且几乎总是漏一项。**

---

## 七、代价

给样本写入加插桩，让调度延迟多出约 **16 周期**：插桩前 267（基线 phase 389），
插桩后 283（基线 399）。1:1 的屏蔽斜率不受影响。

这是这个保证的真实价格，而且它写在 [`EXPERIMENT-02.md`](EXPERIMENT-02.md) 里，不是藏起来的。

---

## 八、怎么新增一个临界区

1. 在 `exp/crit-sections.toml` 加一个 `[[site]]`，写明名字、用途、`budget_cycles`
2. 用生成的索引常量：`crit::mask(crit::SITE_YOUR_NAME, || { ... })`
3. 构建。超过 `max_mask_cycles` 就缩短临界区，或者加 `exempt = true` 并写明理由
4. 上板跑，拿 `observed_max` 和申报值对。**默认你会第一次就填错。**

---

## 九、它不是什么

- **不是 WCET 证明。** 见第二节。
- **不能替代准入控制。** 它管住的是一个危害（屏蔽中断）；CPU 可用性是另一个危害，归限流管。
- **不在运行期触发 trap。** 超限只累加计数器，不中止系统。把它变成断言是有意的下一步而非疏漏 ——
  在一个已经关中断的区域里触发断言，本身就是时序风险来源。
- **不覆盖嵌套或一周期内多段临界区。** 清单是每站点一个数字。
