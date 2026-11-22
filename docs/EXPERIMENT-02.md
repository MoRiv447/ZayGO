# Experiment 02: Interrupt-Masking Sensitivity

> English version. [中文版](EXPERIMENT-02.zh-CN.md)
>
> **Status: run on hardware 2026-11-22.** Eight mask durations, 128 samples each.
> Conclusion: **worst-case release delay grows 1:1 with interrupt-masking duration.**

---

## 1. Why this experiment exists

Experiment 01 measured roughly **zero** interference from three very different load types, and
section 7.4 of [`EXPERIMENT-01.md`](EXPERIMENT-01.md) explained why: the load never disabled
interrupts, and SysTick has the highest priority, so it could always get in.

That left the real question untouched: **what happens when something does disable interrupts?**

This is the dominant threat in real firmware. A driver that holds `cpsid i` for 200 µs while it
bit-bangs a bus will delay every real-time task by up to 200 µs, no matter how good the scheduler is.

---

## 2. Why the metric had to change

Experiment 01's core metric was `t_start - t_release`. **Reusing it here would have produced a
false "zero interference" result.**

When interrupts are masked, SysTick is not lost — it is *deferred*. The pending bit is set, and
the ISR runs the moment `cpsie i` executes. So `t_release` slides forward together with `t_start`,
and the difference stays at ~250 cycles. The metric is blind to the very thing being measured.

So experiment 02 records two quantities instead:

| Metric | Definition | Role |
|---|---|---|
| **`phase`** | `(t_start − origin) mod 64000` | **Core metric.** Offset from the *ideal* release instant, which is fixed by hardware and does not slide with the ISR |
| `dispatch` | `t_start − t_release` | Control. Expected to stay constant, proving the delay comes from "when we start reacting", not from "the switch got more expensive" |

`origin` is the phase at which the first tick landed. It is not zero — see 4.1.

---

## 3. Design: deliberately hitting the worst case

If the critical section starts at a random phase, a mask of M cycles covers the release instant
with probability only `M / 64000`. For M = 500 that is 0.8 % — thousands of samples would be needed
before the worst case appeared once, and the measured `max` would be noise.

So the stimulus is constructed to always hit:

1. Wait until the phase is `PRE` (200) cycles before the release instant.
2. Disable interrupts.
3. **While still masked**, busy-wait until the phase wraps — the release instant has now passed.
4. Stay masked for exactly `M` more cycles.
5. Re-enable interrupts.

The blocked duration past the release instant is therefore exactly `M`, independent of how long
`cpsid i` takes. Total critical-section length is `PRE + M`, which is reported honestly below.

**This measures the worst case, not the average.** A random-phase experiment would answer a
different question.

Note that M = 0 also goes through the same path (pre-window, wrap wait, zero extra mask), so the
baseline shares the stimulus structure and the curve stays a straight line. The absolute floor —
no critical section at all — is experiment 01's number, about 226 cycles.

---

## 4. Three bugs found while building this

All three would have produced plausible-looking but wrong data.

### 4.1 CYCCNT and SysTick are not phase-aligned

`time::init()` zeroes CYCCNT, then `main` enables SysTick a few hundred cycles later. So tick
boundaries fall on `CYCCNT mod 64000 == origin` with `origin ≈ 600`, **not on 0**.

First run, with the wait aligned to 0, gave a baseline `phase` of 830 instead of ~230, and the
500-cycle stage showed no delay at all: the whole masking window was offset by 600 cycles and
missed the release instant.

Fix: record `origin` from the first tick and subtract it everywhere.

### 4.2 Racing the boundary with software does not work

The first fix still failed: every stage showed zero delay. The wait window was 16 cycles, but
`interrupt::free` costs about 20 cycles to actually execute `cpsid i` — by the time interrupts
were really off, the release instant had passed.

**Software cannot chase a hardware boundary.** The fix was to stop trying: disable interrupts
*early* (200 cycles ahead), then wait for the phase wrap *inside* the masked region, and only
then count out the M cycles.

### 4.3 The lost-tick counter lied twice

`overruns` (the "last round had not finished" counter from experiment 01) stays at **0** under
masking, because the hardware collapses multiple pending ticks into one. So a new counter was
needed: compare each tick's actual arrival against the expected arrival.

Two bugs in it, both producing confident-looking nonsense:

| Bug | Reported | Truth |
|---|---|---|
| `EXPECTED` not resynchronised after counting | 4200 | drift was re-counted on every later tick |
| `t - expected` underflow not guarded | 67173 | exactly `2^32 / 64000` — the signature of a negative difference wrapped into u32 |

After both fixes: **64**, which is what arithmetic predicts (128 rounds × ~1.5 periods each, one
ISR per round → ~0.5 tick lost per round).

> The lesson is the same one as in experiment 01: **a counter that returns a plausible number is
> more dangerous than one that crashes.** Both wrong values had to be checked against an
> independent estimate before they could be rejected.

---

## 5. Results

Platform: STM32H723ZGT6 (Cortex-M7 r1p2), **64 MHz HSI, no PLL, cache off**, 1 ms period.
128 samples per stage, `overruns = 0`, `tick_missed = 64` (all from the last stage).

Units are cycles; 1 cycle = 15.6 ns.

| mask (cyc) | n | `phase` min | p50 | p99 | max | `dispatch` p50 | Δ p50 vs M=0 | Δ/mask |
|---|---|---|---|---|---|---|---|---|
| 0 | 128 | 384 | **389** | 445 | 483 | **267** | 0 | — |
| 500 | 128 | 886 | **891** | 912 | 912 | **267** | 502 | **100.4 %** |
| 1000 | 128 | 1390 | **1395** | 1416 | 1416 | **267** | 1006 | **100.6 %** |
| 2000 | 128 | 2371 | **2377** | 2381 | 2381 | **267** | 1988 | **99.4 %** |
| 4000 | 128 | 4385 | **4391** | 4395 | 4395 | **267** | 4002 | **100.1 %** |
| 8000 | 128 | 8391 | **8397** | 8399 | 8411 | **267** | 8008 | **100.1 %** |
| 16000 | 128 | 16371 | **16375** | 16392 | 16392 | **267** | 15986 | **99.9 %** |
| 96000 | 128 | 297 | **32265** | 63457 | 63709 | 281 | — | — |

As time (@64 MHz):

| mask | p50 | max |
|---|---|---|
| 0 | 6.1 µs | 7.5 µs |
| 500 | 13.9 µs | 14.2 µs |
| 1000 | 21.8 µs | 22.1 µs |
| 2000 | 37.1 µs | 37.2 µs |
| 4000 | 68.6 µs | 68.7 µs |
| 8000 | 131.2 µs | 131.4 µs |
| 16000 | 255.9 µs | 256.1 µs |
| 96000 | 504.1 µs | 995.5 µs |

(The last row's p50 ≈ half a period and max ≈ a full period — that is the phase-lock collapse
described in 6.3, not a delay value to be read literally.)

---

## 6. Interpretation

### 6.1 The relationship is 1:1

For every stage up to and including 16000 cycles, `Δ/mask` is between 99.4 % and 100.6 %.

**Disabling interrupts for M cycles delays the worst-case task release by M cycles.** Not
sub-linearly, not with a safety factor — one for one. This is the number a `budget` field has to
be checked against.

The absolute figures: 500 cycles of masking (7.8 µs) turns a 6.1 µs release into 13.9 µs. A driver
that masks for 1000 cycles (15.6 µs) more than triples it.

### 6.2 `dispatch` never moves

`dispatch` is 267 cycles in all seven real stages, and 281 in the pathological last stage. The
cost of *switching* is unaffected by masking; only the moment at which the switch can begin is.

That decomposition is the reason the two metrics are recorded separately. A single "latency" number
hides which of the two effects caused it.

### 6.3 Above one period, phase locking collapses

The 96000-cycle stage (1.5 periods) behaves completely differently: `phase` is spread across the
whole period, p50 at 32265 ≈ half a period, max at 63709 ≈ a full period.

This is not "a longer delay". It is **loss of phase lock**: with ticks being swallowed, the release
instant is no longer related to the data stream at all. The distribution becomes uniform, which is
the statistical signature of an unlocked system.

`tick_missed = 64` confirms it quantitatively.

> For a real-time system this is a qualitative failure, not a quantitative one. Once ticks are
> lost, no amount of scheduling analysis on the tick boundary helps — the task is no longer
> executing at the rate the analysis assumed.

### 6.4 What this means for the architecture

The bandwidth cap discussed in [`PLAN.md`](../PLAN.md) was never the mechanism that protects
against this. Nothing in a rate-limiting scheme prevents a driver from holding `cpsid i`.

What this experiment justifies is a different mechanism:

| Finding | Architectural consequence |
|---|---|
| Delay grows 1:1 with mask duration | The real-time plane's budget must be expressed as **an interrupt-masking allowance**, and the worst case computed as `dispatch + max_mask` |
| `dispatch` is constant and small (267 cyc) | Scheduler overhead is not the problem; do not optimise it |
| Masking beyond one period destroys phase lock | The design must **guarantee** no critical section exceeds the period, and that guarantee has to be checkable — a static rule, not a coding convention |
| A silent counter gave two wrong answers | The build must emit the mask-duration budget as a **report**, and CI must fail when a critical section exceeds it |

The third row is the interesting one: it argues for **enforcing an upper bound on interrupt
masking at build time** — which is a more valuable and more tractable guarantee than bandwidth
capping, and it is exactly the kind of thing that fits a declarative scenario template.

---

## 7. Limitations, stated honestly

1. **Worst case, not expectation.** The critical section is deliberately aligned to the release
   instant. A random-phase experiment would give a distribution from ~0 to M; it would need
   thousands of samples per stage to characterise and is a different (also interesting) question.
2. **The stimulus is synthetic.** A real driver's masking pattern is not a clean rectangle.
3. **64 MHz, no cache.** The absolute microsecond figures do not transfer to 550 MHz. The 1:1
   relationship does, because it is structural rather than microarchitectural.
4. **Single masked region.** Nested or multiple critical sections per period are not covered.
5. **`phase` is relative to the first tick.** Clock drift or a changed SysTick reload would
   invalidate it; the firmware records `origin` once and assumes it is stable.
6. **No interrupt latency measured.** The ISR entry latency itself (roughly 20 cycles here,
   inferred from the 200-cycle pre-window being sufficient) was not measured directly. That is a
   separate experiment.
