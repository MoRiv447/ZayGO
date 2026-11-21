# ZayGO

> English version. [中文版](README.zh-CN.md)
>
> A real-time platform for high-performance MCUs: **fixed application set, tiered extensibility,
> verifiable as a whole.**

Not the fourth general-purpose RTOS. What it tries to solve is turning **predictability** from a
promise in a document into a property the kernel itself can check, can produce evidence for, and
does not need an external tool to verify.

> Status: **P0 complete and verified on hardware.** The code runs, the machine code has been
> checked instruction by instruction, and timing data has been collected.
> Every section marks what is **verified** and what is **not**. The full phase table is in
> [`PLAN.md`](PLAN.md).

---

## Contents

- [What it is not](#what-it-is-not)
- [Why it is needed](#why-it-is-needed)
- [How four constraints combine](#how-four-constraints-combine)
- [Measured results](#measured-results)
- [Where the project stands](#where-the-project-stands)
- [Quick start](#quick-start)
- [Directory layout](#directory-layout)
- [Documents](#documents)

---

## What it is not

Drawing the boundary first, otherwise it gets compared to Zephyr on driver count and to seL4 on
proof size, and loses both times.

| Not doing | Why |
|---|---|
| No full POSIX | Generality comes from **built-in applications plus declarative templates**, not from being able to install arbitrary programs |
| Not chasing driver breadth | 1000+ boards take thousands of person-years; that is not a race one person can win |
| No full-kernel formal verification | seL4 took twenty years; this project verifies only the scheduler's core invariants |
| No runtime loading of native code | That is a precondition for predictability, not a compromise (see below) |

**What it does aim for**: making "this task will not miss its deadline" something that can be
checked at build time and at run time, and that leaves evidence behind.

---

## Why it is needed

The three reference projects are not homogeneous competitors; they are three mutually exclusive
trade-offs. Each one's gap is real:

| | Zephyr 4.5 | seL4 16.0 | NuttX 13.1 |
|---|---|---|---|
| Orientation | breadth first | correctness first | POSIX completeness first |
| Scale | 1000+ boards, 11 architectures, 1200+ contributors in one release | few platforms, full tooling | 300+ boards, 1500+ config templates |
| Timing guarantee | **none** | kernel WCET is verified, **drivers and applications are not** | none |
| Formal verification | none | functional correctness + integrity + confidentiality (AArch64 complete) | none |

Zephyr's own `include/kernel.h` says it plainly in the documentation for `k_thread_deadline_set()`:

> "Despite the API naming, the scheduler makes **no guarantees** the thread WILL be scheduled within that deadline,
> nor does it take extra metadata (like e.g. the 'runtime' and 'period' parameters in Linux `sched_setattr()`)
> that allows the kernel to validate the scheduling for achievability."

A 2026 paper from UCSC puts it even more bluntly:

> "The Zephyr RTOS... **provides no guarantees at all on timing** other than experimental benchmarks."

Three gaps follow from this:

1. **No reconciliation mechanism between generality and predictability** — admission control,
   static partitioning and static timing analysis have no mature open-source MCU implementation.
2. **The "verifiable but sufficient" middle ground is empty** — seL4 is too heavy, NuttX is not
   verified, Zephyr does not guarantee timing.
3. **Mixed-criticality has no engineering implementation** — running a µs-scale control loop
   alongside a ms-scale UI and network stack on one MCU is a real industrial need, and industrial
   RTOSes only offer "basic support".

---

## How four constraints combine

Asking for generality, real-time performance, reliability and predictability at once yields
nothing coherent. The answer is not to maximise each one but to let **each stand on its own
mechanism, with the four mechanisms stacked**:

| Goal | Mechanism | Current state |
|---|---|---|
| **Generality** | L0 built-in apps / L1 declarative scenario templates / L2 bounded Wasm plugins / **L3 native loading forbidden** | designed, not implemented |
| **Predictability** | Fixed application set ⇒ WCET is computable; fuel and memory-page caps ⇒ plugins are bounded; admission control ⇒ over-capacity is refused | **dispatch latency measured**, admission control not implemented |
| **Reliability** | MPU partitioning; Non-Cacheable framebuffer; Rust memory safety; a single hashable artifact | designed, not implemented |
| **Real-time performance** | Bandwidth caps; non-blocking device API; every write is a "submit request" | **minimal scheduler implemented and verified on hardware** |

### The key judgement: compile-time assembly ≠ runtime installation

Forbidding native code installation does not mean giving up generality. **What a template
produces is still a single firmware image, still a hashable and signable artifact, still a finite
and enumerable set of execution paths.** Runtime immutability is fully preserved, but one kernel
can serve many scenarios.

This is not a new invention — ESPHome also compiles its configuration into the firmware rather
than installing it onto the device, and it serves millions of scenarios.

Two traps are deliberately avoided:

| | NuttX | Zephyr | **ZayGO** |
|---|---|---|---|
| Configuration carrier | Kconfig menus | Kconfig + Devicetree (**two sources of truth**) | **a single declarative scenario file** |
| Combination scale | 1500+ templates | tens of thousands of options | a few dozen scenario templates |
| Artifact | a configured firmware | a configured firmware | firmware + **a schedulability report** + SBOM |

### The four planes

```
┌─ Scenario template (the only build-time entry point) ┐   ┌─ Single firmware image ─┐
│ Declarative file                                      │──▶│ Hashable, signable       │
│ Every task declares period / deadline / budget        │   │ Finite, enumerable paths │
└───────────────────────────────────────────────────────┘   └───────────┬──────────────┘
┌─ Runtime · MPU partitioned ───────────────────────────────────────────┴─────┐
│  Real-time plane   exclusive budget, zero dynamic allocation, no waiting   │
│  Display controller L8 indexed framebuffer + 256-entry CLUT per layer       │
│  Management plane  HTTP/WS data out, bandwidth capped, always preemptible │
│  Plugin plane      Wasm sandbox, fuel + memory-page caps                  │
│  Unified device API namespace addressing · non-blocking message passing   │
└────────────────────────────────────────────────────────────────────────────┘
```

### Two verified hardware facts (they decide whether any of this is possible)

- **The STM32H723 has an MPU** (the Cortex-M7 MPU is optional and ST implemented it on H7), so
  static partitioning works and there is no need to move to an M33/M55.
- **The STM32 LTDC natively supports L8 plus a 256×24-bit CLUT per layer**, with no DMA2D
  involvement, so the display plane's CPU cost can drop to nearly zero.

---

## Measured results

What P0 produced: the timing distribution of a self-written minimal preemptive scheduler on an
STM32H723 (**64 MHz HSI, no PLL, cache off**).

8192 samples per mode (1 ms period, about 8.2 seconds), overruns = 0 in every run.

**Dispatch latency (`t_start − t_release`)**

| Mode | Load characteristic | p50 | p99 | p99.9 | max (steady state) |
|---|---|---|---|---|---|
| Idle | 8×nop spin, no memory access | 3.58 µs | 3.66 µs | 3.66 µs | 3.66 µs |
| Spin | 512 multiply/rotate iterations, registers only | 3.53 µs | 3.53 µs | 3.53 µs | 3.67 µs |
| Web | 256-byte branch-dense read + 128 memory writes | 3.66 µs | 3.66 µs | 3.66 µs | 3.66 µs |

**Period jitter**: p50 = 63997–64001 cycles, peak-to-peak about 220 cycles = **3.5 µs**

Three observations:

1. **Load type barely affects dispatch latency** (difference ≤ 2.8%). Going from "does nothing"
   to "512 multiplies per round" to "branch-dense plus memory writes" moves the latency almost
   not at all.
2. **Zero long tail**: `p50 = p99.9 = max`, and in all three modes the steady-state latency fits
   inside a 9-cycle band. For hard real-time, **predictability matters more than a low mean**.
3. **The `max=415–431` values all come from sample 0**, a cold-start artefact (empty I-Cache).
   Not one of the 8191 steady-state samples exceeded 300 cycles.

**This falsifies the original hypothesis**: the criterion was "interference above 10% of the task
period ⇒ the bandwidth cap becomes a cornerstone", and measured interference is roughly 0%.

But that is **not "the bandwidth cap is useless"** — it means the experiment never reached the
scenario the cap exists for. Not one of the dimensions that actually threatens dispatch latency
was covered:

| Threat not covered | Why it was not covered |
|---|---|
| **Interrupt masking duration** | The load never executes `cpsid i`, and SysTick has the highest priority so it can always get in |
| **Bus contention** | No DMA, no SPI, no Ethernet traffic |
| **Cache jitter** | Cache is off after reset |
| **Higher clock** | 64 MHz HSI, not the target frequency |

⇒ The next step is **experiment 02: a sensitivity sweep over interrupt-masking duration** (the
independent variable is masking duration, not load type). That is the real criterion for whether
the bandwidth cap is required.

Full interpretation and experimental limitations: section 7 of
[`docs/EXPERIMENT-01.md`](docs/EXPERIMENT-01.md).

---

## Where the project stands

### Verified (with evidence)

| Item | Evidence |
|---|---|
| `exp01` builds with zero warnings | rustc 1.97.1, `cargo build --release` |
| PendSV context switch is 9 instructions with no compiler-inserted prologue | disassembly checked line by line |
| Vector table hooks up correctly | `0x38 → PendSV`, `0x3C → SysTick` |
| Memory layout fits | initial SP `0x24050000`, `.bss` 74KB / 320KB |
| **Firmware runs on hardware** | 8192 samples per mode across three modes, zero long tail, overruns = 0 |
| SWD link | probe "Flash Pro(CMSIS-DAP)" `0d28:0204` |
| Chip identity | CPUID `0x411FC272` (M7 r1p2), Flash 1024KB, DEV_ID `0x483` |
| On-board peripheral pins | extracted from the schematic, see `docs/HARDWARE-NOTES.md` |

The actual machine code of PendSV:

```
mrs   r0, PSP
stmdb r0!, {r4-r11}
str.w lr, [r0, #-4]!
bl    schedule
ldr.w lr, [r0], #4
ldmia.w r0!, {r4-r11}
msr   PSP, r0
bx    lr
```

### Not verified (stated plainly)

| Item | Status |
|---|---|
| Latency under interrupt masking / bus contention | **not measured** (the goal of experiment 02) |
| Timing with cache enabled | **not measured** |
| Timing at 550 MHz (PLL) | **not measured** |
| MPU partition correctness | not implemented, needs fault injection |
| Wasm plugin layer | not implemented |
| L8 + CLUT display | not implemented (hardware support confirmed) |
| Verus verification | not started |
| Debug port dying on `wfi` | **unresolved** (see the pitfalls section of `exp01/README.md`) |

---

## Quick start

### Dependencies

```bash
rustup target add thumbv7em-none-eabihf
cargo install probe-rs-tools
```

Rust ≥ 1.88 (needed for `#[unsafe(naked)]` + `naked_asm!`).

### Wiring

Four wires: **VTref → 3.3V**, SWDIO, SWCLK, GND. (A floating VTref gives
`No target connected`, which is the number one wiring mistake.)

**No screen, no USB, no serial cable.** Output goes over RTT, reusing the SWD cable you already
need for flashing.

### Confirm the chip part number

```bash
probe-rs chip list | grep -i h723
```

Then edit `--chip` in `exp01/.cargo/config.toml`. The reference board is `STM32H723ZGT6`
(1MB Flash / LQFP144).

### Run

```bash
cd exp01
cargo run --release          # defaults to Mode::Idle (the baseline)
```

After 8192 samples the firmware stops SysTick, prints every sample and halts (the `bkpt` is the
"done" signal, so probe-rs exits with a non-zero code — that is expected).

The three experiment points are selected by the `const MODE` in `src/main.rs`:

| `MODE` | Load | Corresponding design document |
|---|---|---|
| `load::Mode::Idle` | 8×nop spin | experiment 1, baseline |
| `load::Mode::Spin` | pure arithmetic spin | experiment 2, CPU saturated |
| `load::Mode::Web` | parse + framebuffer write | experiment 3, simulated management plane |

### Analyse

```bash
python analysis/parse.py out-idle.csv
```

**The core metric is `t_start - t_release` (dispatch latency), not execution time.** Execution
time only says "how long did it run" and cannot show "how long was it held up" — this is the
most common mistake when measuring real-time behaviour.

---

## Directory layout

```
ZayGO/
├── README.md              ← you are here (English)
├── README.zh-CN.md        中文版
├── PLAN.md                implementation plan: criteria, phases, risks (English)
├── PLAN.zh-CN.md          中文版
├── docs/
│   ├── POSITIONING.md     positioning: evidence from three projects, gaps, options
│   ├── ARCHITECTURE.md    four planes, scenario templates, device API, display backends
│   ├── HARDWARE-NOTES.md  board identity, pin assignment, USB Host power, capacitor selection
│   └── EXPERIMENT-01.md   experiment design, measured results, interpretation
└── exp01/
    ├── src/
    │   ├── context.rs     naked context switch (9 instructions)
    │   ├── sched.rs       two-priority scheduler + SysTick release
    │   ├── time.rs        DWT CYCCNT
    │   ├── load.rs        three loads: Idle / Spin / Web
    │   ├── measure.rs     8192-sample buffer + CSV
    │   └── main.rs        entry point (change `MODE` to switch experiment point)
    └── analysis/parse.py  percentile statistics on the PC
```

---

## Documents

| Document | Contents |
|---|---|
| [`PLAN.md`](PLAN.md) | **Implementation plan**: acceptance criteria, phase table, how each phase is implemented, risk register, explicit non-goals, pre-publication checklist |
| [`docs/POSITIONING.md`](docs/POSITIONING.md) | Evidence from the three reference projects, the shared gaps, candidate routes and trade-offs |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Four planes, dual display backends, scenario templates, Plan 9-style device API |
| [`docs/HARDWARE-NOTES.md`](docs/HARDWARE-NOTES.md) | Board identity, pin assignment, USB Host power circuit, capacitor selection |
| [`docs/EXPERIMENT-01.md`](docs/EXPERIMENT-01.md) | Experiment design, measured results, interpretation and limitations |
| [`exp01/README.md`](exp01/README.md) | Implementation details, **four hardware pitfalls hit**, troubleshooting checklist |

---

## About the name

`ZayGO`. There is no deep meaning behind it — it simply felt right, and some names are like
that. It is not an acronym and it does not stand for anything.

## Licence

Not decided yet. No licence terms are introduced during the design phase.
