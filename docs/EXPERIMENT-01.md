# Experiment 01: Quantifying Interference from a Preemptive Scheduler

> English version. [中文版](EXPERIMENT-01.zh-CN.md)

> Purpose: before writing the kernel, use the minimum amount of code to measure the "actual magnitude of the interference that non-real-time load inflicts on real-time tasks",
> and whether "a bandwidth cap mechanism can push that interference down into an acceptable range".
> This experiment depends on no unresolved design choices, and its conclusions will directly determine which mechanisms in the architecture are necessary.
>
> **Status: running on hardware and passing (2026-11-21), all three data sets complete. Conclusions are in the "Results" section at the end.**

---

## 0. What This Experiment Answers

Three mechanisms in the architecture claim to protect real-time behavior:

| Mechanism | Claimed effect | What the experiment must verify |
|---|---|---|
| Fixed-priority preemption | Real-time tasks always run first | **How large is the switch overhead, really**? At 550MHz / 800MHz, is it hundreds or thousands of cycles? |
| Bandwidth cap | Non-real-time load cannot fill up the CPU | **After rate limiting, how far does real-time task jitter drop**? |
| Isolation (MPU) | Memory errors do not spread | **Not measured** (software behavior, cannot be quantified by timing; covered by later HIL tests) |

**Key unknown**: with no bandwidth limit, how high can a simulated Web load push real-time task latency?
- If the result is tens of microseconds ⇒ bandwidth capping is a nice-to-have
- If the result is a few milliseconds or even tens of milliseconds ⇒ **bandwidth capping is mandatory, not optional**

This number directly determines:
- The isolation strength required between the real-time plane and the display controller
- The form of the admission control criterion (static vs measurement-based)
- How strict the semantics of the `budget` field in the scenario template must be

---

## 1. Experimental Design

### 1.1 Hardware and Tooling

| Item | Choice | Notes |
|---|---|---|
| Platform A | STM32N647 (Cortex-M55 800MHz) | Primary platform; the M55 has the Helium vector extension and a 32KB D-Cache |
| Platform B | STM32H723 (Cortex-M7 550MHz) | Control group; the M7 has no vector extension |
| Kernel | In-house minimal scheduler, Rust `no_std` | No FreeRTOS/Zephyr, so the system under test does not itself introduce interference |
| Debugging | probe-rs + ITM/SWO | You already have experience with probe-rs 0.32.0 |
| Timing | **DWT->CYCCNT** | Both Cortex-M7/M55 have a DWT cycle counter |
| Probe | `probe-rs attach` + RTT or SWO | Small data volume, so RTT is less work |

**Why write our own scheduler instead of using an off-the-shelf one**: what we need to measure is "the interference of the ZayGO scheduler"; measuring with an off-the-shelf kernel measures
"that kernel's interference", and the conclusion does not transfer. The minimal scheduler is about 300 lines and can be written within a week.

### 1.2 Minimal Scheduler Specification

Only three things need to be implemented:

```
1. Two fixed priorities (PRI0 real-time, PRI1 normal)
2. SysTick 1kHz preemptive scheduling
3. Context switch (save/restore registers)
```

**Not implemented**: priority inheritance, time slicing, blocking primitives, memory management, MPU configuration.
(The correctness of MPU isolation is measured separately with HIL fault injection, not in this experiment.)

Context saving for the context switch uses `#[naked]` + `global_asm!`;
this is a known difficulty for preemptive Rust kernels (the automatic stacking on M7/M55 saves only r0-r3/r12/lr/pc/xPSR,
the rest has to be saved by hand).

### 1.3 Three Test Loads

| Load | Description | What it simulates |
|---|---|---|
| **L-A Idle** | Only the real-time task exists, the system nearly idles | Baseline |
| **L-B Full CPU load** | Normal-priority task in a tight infinite loop, never yields | "An out-of-control Web load" |
| **L-C Simulated Web** | Normal-priority task loops: parse a fixed buffer + fill the frame buffer + "send a packet" | A real management plane load |

L-C matters because **a pure infinite loop is the best case** (no cache miss, no branch prediction failure, no memory bandwidth contention).
Real HTTP parsing + frame buffer filling triggers D-Cache misses and memory bandwidth contention,
so the interference will be significantly larger. L-C must be as realistic as possible.

### 1.4 Measurement Method

**Three quantities are recorded per real-time task period**:

```
t_release   : the moment the task is woken (software timestamp)
t_start     : the moment the task actually begins executing
t_end       : the moment the task finishes executing
```

From these:

| Metric | Definition | Meaning |
|---|---|---|
| **Release latency** | `t_start - t_release` | **Core metric**: how long the real-time task was held up by a normal-priority task |
| Execution time | `t_end - t_start` | Whether the load was effective |
| Period jitter | Difference between two consecutive `t_start` values - period | Long-term stability |

**You must use `t_start - t_release`, not `t_end - t_start`**:
the latter measures "how long the task ran", it cannot measure "how long it was held up". Many people get this wrong.

**Sampling requirements**:
- ≥ 10000 consecutive periods (10 seconds @1kHz)
- Record **all** samples, do not record only min/max (the tail is the problem)
- Report percentiles: p50 / p99 / p99.9 / **maximum**
- Write the raw samples into the RTT ring buffer, dump them to the PC after the experiment for distribution analysis

### 1.5 Prototype of the Bandwidth Cap Mechanism

There is no need to implement the full CBS. The prototype only needs:

```
each normal-priority task declares a budget (e.g. 20%)
every tick (1ms) check how much of that budget the task has used
over budget -> the task is suspended for the rest of this tick (forced yield to the real-time task)
```

This is a simplified version of CBS, enough to answer "can rate limiting push the interference down".

### 1.6 Planned experiment matrix, and what was actually run

The matrix below was the design intent. The **Status** column records what really happened, because
the two diverged.

| # | Platform (planned) | Load | Bandwidth cap | Purpose | Status |
|---|---|---|---|---|---|
| 1 | M55 | L-A | — | **Baseline**: how accurate the system is on its own | ✅ run — **on M7 (H723), not M55** |
| 2 | M55 | L-B | none | Worst-case interference | ✅ run — on M7 |
| 3 | M55 | L-C | none | Interference under a realistic load | ✅ run — on M7 |
| 4 | M55 | L-C | 20% | Whether rate limiting works | ❌ not run — no bandwidth cap exists yet |
| 5 | M55 | L-C | 5% | Whether rate limiting worsens starvation | ❌ not run — same reason |
| 6 | M7 | L-A | — | Platform control: switch-overhead difference | ⚠️ subsumed — everything ran on M7 |
| 7 | M7 | L-C | none | Platform control: interference difference | ⚠️ subsumed — same reason |

**Why the platform changed**: the N647 (Cortex-M55) board is not available until the next academic
term, so every run used the H723 (Cortex-M7). The intended M55-versus-M7 comparison is therefore
**deferred, not dropped** — and because the scheduler is 300 lines with no hardware dependencies
beyond the DWT counter, porting it later is a small job.

**Experiments 1 is the control group**: it measures how much jitter the real-time task has on its
own, with no interference. This number is the denominator of every later comparison — **without
it, a claim of "3 ms of interference" cannot be interpreted** (3 ms relative to zero, or relative
to 50 µs?).

---

## 2. How to interpret the result

- Interference < 1% of the real-time task period ⇒ bandwidth capping is downgraded to a
  "defensive measure" and the architectural focus shifts to admission control
- Interference > 10% ⇒ bandwidth capping is promoted to an "architectural cornerstone": the
  `budget` field must be validated at build time, and the scenario template must reject
  declarations that are mathematically impossible
- Between 1% and 10% ⇒ tier by criticality: the real-time plane gets exclusive bandwidth, the
  management plane shares it, the plugin plane is capped

The measured numbers landed at roughly 0%, so the first branch applies — **but read 7.4 before
acting on that**, because it also means the experiment never reached the regime this rubric was
written for.

---

## 3. Risks: anticipated versus what actually happened

| Risk anticipated at design time | Mitigation planned | What actually happened |
|---|---|---|
| DWT->CYCCNT inaccurate once D-Cache is on | Put the measurement code in ITCM, or read after `__DSB()` | Not exercised — cache was left off, so this stays untested |
| RTT transfer jitter pollutes the measurement | Sample into a RAM ring buffer, dump afterwards | Sequence was right, but **the dump silently dropped about 30% of the samples** — see the RTT pitfall in `exp/README.md` |
| Context switch written wrong (state not fully saved) | Canary variables to validate after switching back | **This is the bug that actually hit** — though not through canaries. It was found by reading `CFSR` and the saved context. The canaries were never added |
| SysTick 1 kHz itself jitters | Timestamp immediately at release, never defer | Fine: period p50 came out at exactly 64,000 cycles |
| Compiler optimisation changes the timing | `#[inline(never)]` + volatile on the critical path | Fine: the naked function was verified by disassembly |
| The infinite-loop load blocks observation | Break in via SysTick, no cooperative yield | Fine |

---

## 4. What This Experiment Does Not Answer

Stated honestly, to avoid later misuse:

- **It does not measure the correctness of memory isolation**. The effectiveness of MPU partitioning has to be proven by fault injection testing (HIL), not by timing measurement.
- **It does not measure the soundness of WCET**. DWT measures the execution time distribution, not the worst-case upper bound;
  a sound upper bound requires static analysis or an MBTA + EVT argument.
- **It does not measure the real impact of bus contention**. L-C is a software simulation, which is not the same as real SPI/Ethernet traffic.
- **It does not measure scheduler invariants**. That is Verus's job.
- **Single-core conclusions**. On multi-core (M7 has an M4 core, N6 does not) the cache/bus contention model differs, a separate experiment is needed.

---

## 5. What was actually delivered

```
exp/
├── README.md / README.zh-CN.md
├── Cargo.toml / Cargo.lock
├── build.rs
├── memory.x                 # flash / RAM regions
├── .cargo/config.toml       # target, runner, --chip
├── src/
│   ├── main.rs              # entry point, pick the experiment point
│   ├── sched.rs             # minimal preemptive scheduler
│   ├── context.rs           # naked context switch
│   ├── time.rs              # DWT->CYCCNT wrapper
│   ├── load.rs              # Idle / Spin / Web loads
│   └── measure.rs           # sample buffer + CSV dump
├── analysis/
│   └── parse.py             # percentile statistics and interpretation
└── out-{idle,spin,web}.csv  # raw measured data, committed as evidence
```

Two files that the design sketch listed were not built, and the reason matters:

- **`src/rtt.rs`** — RTT output ended up in `measure.rs`, because the dump is inseparable from
  the sample buffer and splitting them would only have added a module boundary for its own sake.
- **`scripts/`** — flashing is just `cargo run`, since the runner in `.cargo/config.toml` already
  invokes probe-rs. A shell script around it would have been one more thing to keep in sync.

## 6. Execution order: planned versus actual

The design said: run 1 alone first, then 2, then 3, then 4/5, then 6/7.

What actually happened:

| Step | Planned | Actual |
|---|---|---|
| 1 | Baseline only, confirm the numbers are sane | ✅ done. **It immediately caught a real bug** — see 7.4 |
| 2 | Full CPU load | ✅ done |
| 3 | Simulated Web load | ✅ done |
| 4/5 | Rate limiting at 20% and 5% | ❌ skipped: there is no bandwidth cap to test yet |
| 6/7 | M7 platform control | ⚠️ moot: every run was already on M7 |

**The "baseline first" rule earned its place.** Running the baseline alone is what surfaced the
swapped `SHPR3` bit fields: without a clean baseline, the resulting HardFault would have looked
like a context-switch bug rather than a priority-configuration bug, and the search would have
started in the wrong file.

---

## 7. Results (Measured 2026-11-21)

Platform: Lu Xiaoban LXB723ZG-P1, STM32H723ZGT6, Cortex-M7 r1p2, **64 MHz HSI (no PLL configured)**,
I-Cache / D-Cache not enabled, probe Flash Pro(CMSIS-DAP).

8192 samples per mode (1 ms period, about 8.2 seconds).

### 7.1 Dispatch Latency (`t_start − t_release`, unit: cycles / µs)

| Mode | Load signature | Cold start | min | p50 | p99 | p99.9 | max | mean |
|---|---|---|---|---|---|---|---|---|
| Idle | 8×nop idle spin, no memory access | 431 | 229 | 229 | 234 | 234 | **234** | 231.5 |
| Spin | 512 multiplies/loop, pure registers | 415 | 222 | 226 | 226 | 226 | **235** | 226.0 |
| Web | 256-byte branch-heavy read + 128 memory writes | 431 | 229 | 234 | 234 | 234 | **234** | 232.3 |

Converted to time (@64 MHz, 1 cycle = 15.6 ns):

| Mode | p50 | max | mean |
|---|---|---|---|
| Idle | 3.58 µs | 3.66 µs | 3.62 µs |
| Spin | 3.53 µs | 3.67 µs | 3.53 µs |
| Web | 3.66 µs | 3.66 µs | 3.63 µs |

### 7.2 Period Jitter (difference between two consecutive `t_start` values)

| Mode | p50 | min | max | Peak-to-peak |
|---|---|---|---|---|
| Idle | 63998 | 63791 | 64013 | 222 cycles = **3.5 µs** |
| Spin | 63997 | 63791 | 64011 | 220 cycles = **3.4 µs** |
| Web | 64001 | 63777 | 64013 | 236 cycles = **3.7 µs** |

### 7.3 Interpretation

**Conclusion 1: steady-state dispatch latency ≈ 3.5–3.7 µs (226–234 cycles), the spread across the three loads is ≤ 2.8%.**

The load changes from "doing nothing" to "512 multiply-adds per round", then from "pure arithmetic" to "branch-heavy + memory writes",
and the latency barely moves. **The load type is not a factor in dispatch latency.**

**Conclusion 2: no long tail.** The `p50`, `p99`, `p99.9` and `max` of all three modes all fall within a single band of 226–235 cycles, 9 cycles wide (Web's min=229 differs from p50=234 by only 5 cycles). For hard real-time,
**predictability is worth more than a low mean, and this data set has a zero long tail.**

**Conclusion 3: `max=415–431` comes entirely from sample 0, it is a cold start artifact, not interference.**
At the first dispatch the I-Cache is empty and the branch predictor is not warmed up, so it is nearly twice as slow. Not one of the 8191 steady-state samples
exceeds 300 cycles. **Mixing sample 0 into the statistics inflates max by 84% and hides the real distribution** —
that is also why `analysis/parse.py` now lists it separately.

**Conclusion 4: why the load has no effect at all — because it never disables interrupts.**
SysTick has the highest priority (`SHPR3[31:24]=0x00`), PendSV the lowest (`[23:16]=0xF0`),
the load task never executes `cpsid i`, never holds a lock, and never occupies the bus for long. Under those conditions,
**interrupts can come in at any moment, and what the load is doing is irrelevant.**

### 7.4 This Result Falsifies the Original Hypothesis

The criterion in the original design was "unlimited interference > 10% of the task period ⇒ bandwidth capping is promoted to an architectural cornerstone".
The measured interference is about 0%, so **under that criterion, bandwidth capping is not a requirement for "dispatch latency"**.

But this is not "bandwidth capping is useless", it is **that this experiment never hit the scenario it was designed for**.
Not one of the real threat dimensions was covered:

| Threat | Why it was not measured | How to measure it |
|---|---|---|
| **Interrupt masking duration** | The load never executes `cpsid i` | Have the load enter a critical section that masks interrupts for N cycles, and sweep N |
| **Bus contention** | No DMA, no SPI, no Ethernet traffic | Attach real peripherals and run real traffic |
| **Cache jitter** | After reset the cache is off by default | Re-measure with I/D-Cache enabled |
| **Long workload** | The real-time task's work is only 32 additions | Extend it to approach the period boundary |
| **High clock frequency** | 64 MHz HSI | Configure the PLL up to 550 MHz (cache and wait states all change) |

⇒ **The next step should be experiment 02: a sensitivity sweep over interrupt masking in critical sections** (the axis is masking duration, not load type).
That is the real criterion for "is bandwidth capping mandatory".

### 7.5 Known Experimental Limitations (Stated Honestly)

1. **Only dispatch latency was measured** (release to start of execution), not the task's response time.
2. **64 MHz, no cache, no PLL**: the absolute numbers cannot be extrapolated to a real product running at 550 MHz.
   However, the qualitative conclusion "the load type does not affect dispatch latency" does not depend on the frequency.
3. **Single core, no concurrent DMA**: the CPU is the only master on the bus.
4. **Idle mode is no longer truly idle**: the original design used `wfi`, but as measured, as soon as it entered Sleep the debug port lost contact
   (see the pitfall record in `exp/README.md`), so it was changed to an 8×nop idle spin. The cost is non-zero power,
   the benefit is that WFI wake-up latency is excluded and all three modes are directly comparable.
