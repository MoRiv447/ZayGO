# Experiment 03: Re-running with L1 Caches Enabled

> English version. [中文版](EXPERIMENT-03.zh-CN.md)
>
> **Status: run on hardware 2026-11-22.** Three configurations (off / I-Cache / I+D-Cache), same
> masking sweep, 1024 samples each.
> Conclusion: **caching cuts the constant by 57 % and leaves the slope untouched.**

---

## 1. Question

Experiments 01 and 02 were both run with caches **off**, because that is the reset default on
STM32H7 (DM00272913). Real products turn them on.

Two things could happen, and they mean very different things for the architecture:

| If caching… | Then |
|---|---|
| changes the **slope** of the masking relationship | the 1:1 budget rule is cache-dependent, and every configuration needs its own curve |
| changes only the **constant** | the 1:1 rule is structural; only the fixed overhead needs re-measuring per configuration |

Experiment 02's own limitations section predicted the second. This experiment tests it.

---

## 2. Method

Three builds, same firmware, only `CACHE_MODE` differs:

| Mode | I-Cache | D-Cache |
|---|---|---|
| `Off` | ✗ | ✗ |
| `I` | ✓ | ✗ |
| `ID` | ✓ | ✓ |

Each run repeats the full 8-stage masking sweep (128 samples per stage). Comparing `I` against
`Off` isolates code fetch; comparing `ID` against `I` isolates data access.

### The D-Cache problem that had to be solved first

Enabling D-Cache breaks RTT output, silently.

RTT works by writing characters into a RAM ring buffer that probe-rs reads **directly out of RAM
over SWD**. With D-Cache on, those writes sit in cache as dirty lines; probe-rs reads the stale
physical RAM. The symptom is missing or reordered lines with **no error anywhere**.

The fix deliberately gives up the cache for the output path only:

```rust
fn stop_tick() {
    /* stop SysTick */
    // clean the whole D-Cache back to RAM, then turn it off.
    // ARM requires clean-before-disable: clearing the enable bit alone leaves dirty
    // lines that are simply lost (on M7 that is unpredictable, not auto-writeback).
    cache::prepare_for_rtt();
}
```

Sampling runs with caches on; the dump runs without. The dump is not a timing-critical path, so
slowing it down costs nothing.

The clean/invalidate loop reads sets and ways from `CCSIDR` rather than hard-coding 16 KB / 4-way,
because a hard-coded loop silently covers half the cache on a different part.

---

## 3. Results

Platform: STM32H723ZGT6 (Cortex-M7 r1p2), 64 MHz HSI, no PLL, 1 ms period. 128 samples per stage,
`overruns = 0`.

`phase` p50 by mask duration:

| mask (cyc) | Off | I | I+D | `dispatch` Off | I | I+D |
|---|---|---|---|---|---|---|
| 0 | 399 | 256 | **170** | 283 | 197 | **122** |
| 500 | 918 | 758 | **671** | 283 | 197 | **122** |
| 1000 | 1407 | 1262 | **1175** | 283 | 197 | **122** |
| 2000 | 2400 | 2263 | **2175** | 283 | 197 | **122** |
| 4000 | 4419 | 4261 | **4175** | 283 | 197 | **122** |
| 8000 | 8419 | 8262 | **8175** | 283 | 197 | **122** |
| 16000 | 16410 | 16261 | **16175** | 283 | 197 | **122** |
| 96000 | 33047 | 31773 | 32063 | 294 | 215 | 128 |

Δ/mask, i.e. the slope:

| Config | 500 | 1000 | 2000 | 4000 | 8000 | 16000 |
|---|---|---|---|---|---|---|
| Off | 103.8 % | 100.8 % | 100.0 % | 100.5 % | 100.2 % | 100.1 % |
| I | 100.4 % | 100.6 % | 100.4 % | 100.1 % | 100.1 % | 100.0 % |
| I+D | 100.2 % | 100.5 % | 100.2 % | 100.1 % | 100.1 % | 100.0 % |

---

## 4. Interpretation

### 4.1 The slope does not move

Every configuration gives 100.0 %–100.6 %. **The 1:1 relationship is structural, not
microarchitectural**, which is exactly what experiment 02 predicted it would be.

That matters practically: the masking budget rule does not need a separate calibration per cache
configuration. One rule, one number.

### 4.2 The constant moves a lot, and splits nearly evenly

`dispatch` falls 283 → 197 → 122 cycles: a 57 % reduction in total.

| Step | Cycles saved | Share of total |
|---|---|---|
| Off → I | 86 | 53 % |
| I → I+D | 75 | 47 % |

Code fetch and data access contribute almost equally. Reporting only "I+D saves 161 cycles"
would have hidden that — which is why the three-way split was worth two extra runs.

The remaining 122 cycles is the floor for this switch path at 64 MHz: exception entry, PendSV,
`schedule`, task entry, one `CYCCNT` read.

### 4.3 The baseline `phase` falls further than `dispatch`

399 → 170 is −229, while `dispatch` only accounts for −161. The extra −68 comes from the
stimulus path itself (`mask_round`'s wait loop and wrap detection) also running from cache.

This is a reminder that `phase` and `dispatch` are not "the same number plus a constant" — they
have different compositions, and the difference changed by 68 cycles between configurations.

### 4.4 Consequence for the budget mechanism

The interrupt-masking budget in [`MASK-BUDGET.md`](MASK-BUDGET.md) is expressed in **cycles of
masking**, and that number is unaffected by caching. Good — the rule survives a configuration
change.

But `max_mask_cycles = 800` was chosen as a fraction of the 64000-cycle period, and the *total*
release delay is `dispatch + mask`. With caching on, the fixed part is 122 instead of 283, so the
same mask budget leaves more margin. The honest statement is:

> the mask budget is cache-independent; the resulting worst-case delay is not.

---

## 5. Limitations

1. **Both L1 caches are 16 KB / 32-byte lines** on this part, and the workload is tiny compared to
   them. A workload that thrashes the cache would show a different picture; this experiment does
   not characterise eviction behaviour.
2. **No MPU configuration.** The framebuffer / DMA coherency question from
   [`HARDWARE-NOTES.md`](HARDWARE-NOTES.md) is untouched; that belongs to P6.
3. **The dump path is measured with caches off**, by construction. That is a deliberate
   concession for RTT visibility, not an accident.
4. **64 MHz.** The absolute cycle counts are for the reset-default clock. The 57 % reduction is
   likely larger at 550 MHz, where flash wait states dominate far more — but that is a guess, not
   a measurement.
5. **`96000` remains the phase-lock-lost stage**; its p50 (~half period) is not a delay figure and
   should not be compared across configurations.
