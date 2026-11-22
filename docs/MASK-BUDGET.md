# Interrupt-Masking Budget: Enforced at Build Time

> English version. [中文版](MASK-BUDGET.zh-CN.md)
>
> **Status: implemented and exercised on hardware 2026-11-22.**
> This is the first deliverable of the project's "predictability" column that is a working
> mechanism rather than a measurement.

---

## 1. Why this exists

Experiment 02 measured that worst-case task release delay grows **1:1** with interrupt-masking
duration (99.4 %–100.6 % across seven stages, see [`EXPERIMENT-02.md`](EXPERIMENT-02.md)).

So the answer to "how late can this firmware be?" is simply **the longest critical section in it**.

That number is too important to live in code comments. It has to be written down, checked,
reported, and measured.

---

## 2. Two layers, honestly scoped

| Layer | When | Checks | Cannot catch |
|---|---|---|---|
| `build.rs` | build time | declared budget vs the allowed maximum | a section declared as 120 that actually runs 1200 |
| `src/crit.rs` | run time | measured duration vs the declared budget | code paths that were never executed |

**Neither layer is a WCET proof.** Cache, pipeline, compiler optimisation and accumulated error
can all make the measured value differ from the true worst case. This is **budget monitoring**, not
timing verification. Saying otherwise would be the same mistake this project keeps calling out in
other RTOSes.

What the two layers do buy:

- the number is **declared in one place** instead of scattered
- an over-budget declaration **fails the build**, so it cannot be merged by accident
- a mis-declaration (declared too low) is **caught by measurement**, not by review

---

## 3. The manifest

`exp/crit-sections.toml`:

```toml
[budget]
max_mask_cycles = 800     # the whole firmware's allowance
fail_on_violation = true  # default true; a warning-only check is barely a check

[[site]]
name = "sample_ring_push"
file = "src/measure.rs"
purpose = "writing one sample must not be observed half-done"
budget_cycles = 120

[[site]]
name = "sweep_mask_stimulus"
file = "src/load.rs"
purpose = "experiment 02's synthetic stimulus — deliberately huge"
budget_cycles = 97000
exempt = true
```

`exempt = true` skips the build check but **still appears in both reports**, so an exemption is
visible rather than quietly forgotten.

The parser is hand-written (no TOML dependency) but **rejects unknown keys**, because a misspelled
key being silently ignored is exactly the failure mode this project keeps hitting.

---

## 4. The build check

If any non-exempt site declares more than `max_mask_cycles`, the build stops:

```
中断屏蔽预算超限 —— 构建失败

  站点 `sweep_mask_stimulus` 申报 96200 周期，超过上限 800 周期（超出 95400）

为什么是硬失败而不是警告：实验 02 测得最坏释放延迟与屏蔽时长 1:1 增长，
一个超限的临界区会直接让实时任务错过截止时间。
要么缩短临界区，要么在 crit-sections.toml 里显式 exempt = true 并写明理由。
```

(Verified by temporarily clearing `exempt` and running `cargo build --release`.)

Every successful build also prints a report, which CI archives:

```
[mask-budget] max_mask_cycles = 800
[mask-budget] longest declared non-exempt critical section = 120
[mask-budget] headroom = 680 cycles
[mask-budget] sites:
[mask-budget]   sample_ring_push             120 cycles  把一条样本写入环形缓冲…
[mask-budget]   sweep_mask_stimulus        97000 cycles  EXEMPT  实验 02 的合成激励…
```

---

## 5. The runtime check

`crit::mask(site, || { ... })` executes the body with interrupts disabled and records both the
maximum observed duration and a violation count.

Reading the code is worth it, because the **measurement window** was wrong twice:

| Version | What it measured | Symptom |
|---|---|---|
| Timing outside `interrupt::free` | critical section **plus** any pending ISR that runs on exit | reported 214 cycles for a body that costs 54 |
| Timing inside, as shipped | the body only | 54 cycles, stable |

Shipped form:

```rust
pub fn mask<F: FnOnce()>(site: usize, f: F) {
    let mut elapsed = 0u32;
    cortex_m::interrupt::free(|_| {
        let t0 = time::now();
        f();
        elapsed = time::elapsed(t0);   // inside: a pending ISR cannot inflate this
    });

    SITE_MAX[site].fetch_max(elapsed, Ordering::Relaxed);
    if elapsed > SITE_BUDGETS[site] {
        SITE_VIOLATIONS[site].fetch_add(1, Ordering::Relaxed);
    }
}
```

Output from a real run:

```
# mask budget: declared_max=800 cycles
# site,budget,observed_max,violations
# sample_ring_push,120,54,0
# sweep_mask_stimulus,97000,96043,0
```

---

## 6. What it actually caught

**Two of my own mis-declarations**, in the first hour.

| Site | Declared | Measured | Outcome |
|---|---|---|---|
| `sweep_mask_stimulus` | 96200 (from `PRE + M`) | 96912 | 125 violations — the derivation missed ~700 cycles of wrapper and polling overhead |
| `sweep_mask_stimulus` | 97000 (from measurement) | 96043 | 0 violations |

The declared value had to be taken from measurement, not derivation. That is the whole argument
for having a runtime layer at all: **a derivation is a hypothesis, and it is usually missing a term.**

---

## 7. Cost

Instrumenting the sample write added about **16 cycles** to dispatch latency: the pre-instrumentation
run measured 267 (baseline phase 389), the instrumented run 283 (baseline 399). The 1:1 masking
slope is unaffected.

That is the honest price of the guarantee, and it is now visible in [`EXPERIMENT-02.md`](EXPERIMENT-02.md)
rather than hidden.

---

## 8. How to add a critical section

1. Add a `[[site]]` block to `exp/crit-sections.toml` with a name, a purpose and a `budget_cycles`.
2. Use the generated index constant: `crit::mask(crit::SITE_YOUR_NAME, || { ... })`.
3. Build. If it exceeds `max_mask_cycles`, either shorten the critical section or add
   `exempt = true` with a written reason.
4. Run on hardware and check `observed_max` against your declared value. **Expect to be wrong the
   first time.**

---

## 9. What this is not

- **Not a WCET proof.** See section 2.
- **Not a substitute for admission control.** It bounds one hazard (masking); CPU availability is a
  separate hazard that a rate limiter addresses.
- **Not enforced at runtime by a trap.** A violation increments a counter; it does not stop the
  system. Turning it into an assertion is a deliberate next step, not an oversight — an assertion
  inside an already-masked region is itself a source of timing risk.
- **Not covering nested or multiple sections per period.** The manifest has one number per site.
