# Experiment 01: Quantifying Interference in a Preemptive Scheduler

> English version. [中文版](README.zh-CN.md)

Before writing a full kernel, use a minimal runnable preemptive scheduler to measure:
**how much does a low-priority load drag the dispatch latency of a high-priority task?**

That number decides whether the bandwidth cap in the architecture is a **required mechanism**
or an optional optimisation.

## Why write our own scheduler

A number measured on FreeRTOS / Zephyr is **that kernel's** interference figure and stops
being true the moment you switch kernels. Only a self-written minimal implementation lets you
attribute the interference cleanly.

This implementation deliberately keeps exactly three things: two fixed priorities, SysTick
preemption at 1 kHz, and PendSV context switching. No time slicing, no blocking primitives,
no memory management.

## Hardware and tooling

| Item | Requirement |
|---|---|
| Board | STM32H723 core board |
| Debug probe | **DAPLink (CMSIS-DAP) or ST-Link** — anything probe-rs drives |
| Software | Rust ≥ 1.88 (needs `#[unsafe(naked)]`), probe-rs, `thumbv7em-none-eabihf` target |

**No screen, no USB, no serial cable.** Output goes over RTT, i.e. the same SWD cable used
for flashing.

### Probe wiring (DAPLink / ST-Link, 4 required + 1 recommended)

| Probe | → | H723 | Note |
|---|---|---|---|
| **VTref** | → | **3.3V** | **Must be connected.** Level reference only, it does not supply power. Floating ⇒ `No target connected` |
| SWDIO | ↔ | PA13 | bidirectional |
| SWCLK | → | PA14 | |
| GND | ↔ | GND | common ground |
| nRESET | → | NRST | recommended, so the board auto-resets and runs after flashing |

**Do not power the board from two sources**: if the core board already has its own supply,
VTref is only for detection — do not also push the probe's 3.3V into the board (current backfeed).

**RTT does not need the SWO pin** — RTT works by reading target memory over SWD, so a cheap
DAPLink without SWO is fine.

### Probe troubleshooting

| Symptom | Cause |
|---|---|
| `probe-rs list` is empty | VTref not connected, or GND not shared |
| `No target connected` | VTref floating / target not powered |
| `multiple probes found` | more than one probe attached, needs `--probe` |
| `parity error` / unstable link | SWD clock too fast, add `--speed 1000` |
| Flashes fine but no RTT output | wrong `--chip`, so the RTT control block address is wrong |

### Common probe VID:PID

| Probe | Argument |
|---|---|
| DAPLink (official ARM mbed) | `0d28:0204` |
| ST-Link V2 | `0483:3748` |
| ST-Link V2-1 / V3 | `0483:374b` / `0483:374e` |

`probe-rs list` shows the actual value; trust that over any table.

## One-time setup

```bash
rustup target add thumbv7em-none-eabihf
cargo install probe-rs-tools
```

**Confirm the chip part number** (the `runner` line in `.cargo/config.toml`):

```bash
probe-rs chip list | grep -i h723
```

Common:
- `STM32H723ZGTx` — LQFP144
- `STM32H723VGTx` — LQFP100

Change `--chip STM32H723ZGTx` in `.cargo/config.toml` to match your part.
**A wrong value shows up as "chip not found" during flashing, or a scrambled memory map.**

## Running the experiment

```bash
# 1. Baseline (must be run first)
#    set MODE in src/main.rs to load::Mode::Idle
cargo run --release > out-idle.csv

# 2. CPU saturated
#    MODE = load::Mode::Spin
cargo run --release > out-spin.csv

# 3. Simulated Web load
#    MODE = load::Mode::Web
cargo run --release > out-web.csv

# Analysis
python analysis/parse.py out-idle.csv
```

After 8192 samples (about 8.2 seconds) the firmware stops SysTick, prints every sample and halts.

## Code layout

| File | Responsibility |
|---|---|
| `src/context.rs` | **naked context switch**. 9 instructions of PendSV assembly, nothing inserted by the compiler |
| `src/sched.rs` | Two-priority scheduler, SysTick release logic, interrupt priority configuration |
| `src/time.rs` | DWT CYCCNT wrapper |
| `src/load.rs` | Three load generators (Idle / Spin / Web) |
| `src/measure.rs` | Sample buffer and CSV output |
| `src/main.rs` | Entry point, **switch experiment point by changing the `MODE` constant** |

## Key design decisions (each with a reason)

### 1. Run on the reset-default 64 MHz HSI, do not configure the PLL

A misconfigured PLL hangs the chip with no output at all, which is extremely expensive to debug.
This experiment measures a **relative difference** (with load vs without), and 64 MHz versus
550 MHz does not change the direction of the conclusion.

If absolute numbers are needed later, add the PLL configuration and update `time::CPU_HZ` at
the same time.

### 2. No FPU anywhere

The moment a task uses a floating-point instruction, the hardware does lazy stacking, the
exception frame grows by S0-S15 + FPSCR, `EXC_RETURN` has to change from `0xFFFFFFFD` to
`0xFFFFFFE9`, and the whole stack layout has to be recomputed. To keep the context-switch
assumptions simple, this firmware uses integer arithmetic only.

### 3. The core metric is `t_start - t_release`, not execution time

Execution time only says "how long did the task run" and cannot show "how long was it held
up". This is the most common mistake when measuring real-time behaviour — the numbers look
fine while the system has already missed its deadline.

### 4. PendSV priority must be the lowest

`SHPR3` bits[23:16] = 0xFF. After reset every exception priority is 0 (highest), and without
an explicit change the context switch can land in the middle of interrupt handling and corrupt
the task stack.

### 5. The resume point sits right next to the timestamp

The body of `rt_task` starts with `yield_now()`, so the first instruction after the task
resumes is the one that reads `now()`. Anything in between (such as `while !released`) would
be counted as measurement error.

## Verified (2026-11-21)

- `cargo build --release` passes (rustc 1.97.1)
- PendSV disassembly confirms **9 instructions, no compiler-inserted prologue or epilogue**:
  ```
  mrs r0, PSP
  stmdb r0!, {r4-r11}
  str.w lr, [r0, #-4]!
  bl schedule
  ldr.w lr, [r0], #4
  ldmia.w r0!, {r4-r11}
  msr PSP, r0
  bx lr
  ```
- Vector table offset 0x38 → `0x08000459` (PendSV), 0x3C → `0x08000477` (SysTick)
- Initial SP = `0x24050000` (end of RAM), `.bss` 74KB / 320KB
- **Runs on hardware: all three modes collected 8192 samples each, overruns = 0**

### Measured results (64 MHz, no PLL, cache off)

| Mode | p50 | p99.9 | max (steady state) | mean |
|---|---|---|---|---|
| Idle | 229 cyc / 3.58 µs | 234 | 234 | 231.5 |
| Spin | 226 cyc / 3.53 µs | 226 | 235 | 226.0 |
| Web | 234 cyc / 3.66 µs | 234 | 234 | 232.3 |

- **Load type barely affects dispatch latency** (difference ≤ 2.8%)
- **p50 = p99.9 = max, zero long tail**
- The 415–431 cycle first dispatch is a cold-start artefact (empty I-Cache), excluded from the statistics
- Period jitter peak-to-peak is about 220 cycles = 3.5 µs

Interpretation and experimental limitations: see section 7 of `docs/EXPERIMENT-01.md`.

## Four pitfalls hit while bringing this up on hardware (all fail silently)

### Pitfall 1: SHPR3 bit fields swapped → HardFault on the very first interrupt

The layout of `SHPR3` (`0xE000ED20`):

| Bit field | Exception |
|---|---|
| `bits[23:16]` | **PendSV** (exception 14) |
| `bits[31:24]` | **SysTick** (exception 15) |

Swapping them does not merely cost some performance, it **crashes**:

1. `yield_now()` sets PendSV pending inside SysTick
2. PendSV now has the highest priority → it **preempts SysTick immediately**, forming a nested exception
3. The stacked frame is then a Handler-mode frame (on MSP), but the naked handler only saves r4-r11 to **PSP**
4. The context is wrong → `bx lr` picks up `0xFFFFFFF1` (return to Handler mode) → **INVPC**

**Fingerprint**: the first word of the saved context is `0xFFFFFFF1` instead of `0xFFFFFFFD`,
and the saved `r7` sits right below the top of MSP (an address like `0x2404FFB8`).

**How to diagnose**: read `CFSR` (`0xE000ED28`) → UFSR bit2 = INVPC; then read `SHPR3` to see
the actual priorities; finally dump the task's saved context.

### Pitfall 2: `wfi` makes the SWD debug port disappear

`DBGMCU_CR.DBG_SLEEP` (bit 0) is 0 after reset, so the debug clock stops the moment the core
enters WFI:

```
WARN  Could not clear all hardware breakpoints: ... FAULT response
Error: An ARM specific error occurred.
```

Afterwards even `probe-rs read` fails; only `probe-rs reset --connect-under-reset` recovers it.

Setting `DBGMCU_CR` (`0x5C001004` |= 1) was attempted and **did not fix it**; the cause is
still unknown (possibly H7 has a separate debug clock switch in the D3 domain).

⇒ Idle mode now uses an 8×nop spin instead. Bonus: it excludes WFI wake-up latency and keeps
all three modes comparable on the same footing.

> **DBGMCU base address**: on H7 it is `0x5C001000`, **not** the F4/F7 `0xE0042000`
> (the latter reads back as all zeros on this chip).

### Pitfall 3: RTT's default mode silently drops packets

`rtt_init_print!()` defaults to **`NoBlockSkip`** — packets are dropped when the host cannot
keep up. Measured: 8192 samples came out as roughly 5690, **with no error whatsoever**.

Fix: `rtt_init_print!(rtt_target::ChannelMode::BlockIfFull, 4096)`

- The macro parameter is `$mode:path`, so `BlockIfFull` alone does not compile
- The blocking only happens during the output phase after sampling ends, so it cannot pollute
  any measured value

### Pitfall 4: probe-rs prefixes RTT output with a timestamp

The actual output is `21:01:17.115: 0,431,0`, not `0,431,0`.
`parse.py` must first strip `^\d{2}:\d{2}:\d{2}\.\d+:\s*` with a regular expression,
otherwise every `int()` raises and all samples are silently skipped as noise.

## First-bring-up troubleshooting checklist

| Symptom | Most likely cause |
|---|---|
| No output at all | RTT control block not detected; check `--chip` is right and `cargo run` is actually using probe-rs |
| Startup banner but no samples | SysTick not enabled, or `SHPR3` priorities wrong so PendSV never fires |
| Immediate HardFault | stack top not 8-byte aligned; or wrong `EXC_RETURN` (used 0xFFFFFFE9) |
| All samples are 0 | `DEMCR.TRCENA` not set, so CYCCNT does not count |
| Far fewer than 8192 samples | overruns occurred; check the `overruns=` counter in the output |

## What this experiment does not answer

- **Memory isolation correctness**: MPU partitioning needs fault injection, not timing measurement.
- **Soundness of the WCET**: DWT measures an execution-time distribution, not a worst-case upper bound.
- **Real bus contention**: the `Web` mode is a software simulation, not real SPI/Ethernet traffic.
- **Scheduler invariants**: that is Verus's job.
- **Multi-core conclusions**: the cache/bus contention model differs on multiple cores.

## Next step

After running all three modes, put the three `max` values side by side:

| Result | Architectural meaning |
|---|---|
| Unrestricted interference < 1% of the period | bandwidth cap downgrades to a defensive measure; focus shifts to admission control |
| Unrestricted interference > 10% of the period | bandwidth cap becomes a cornerstone; the `budget` field must be validated at build time |
| In between | criticality must be tiered: real-time plane exclusive, management plane shared, plugin plane capped |

The measured numbers came out at roughly 0% interference, which **falsifies the premise of the
original criterion** — see section 7 of `docs/EXPERIMENT-01.md` for why, and what the real
threat dimensions are.

---

## P1: serial CLI and the device API

Plan 9-style naming, non-blocking semantics, no allocator.

### Devices

| Path | Access | Behaviour |
|---|---|---|
| `/stat/sched` | read | `cpu_hz`, `overruns`, `tick_missed`, `samples` |
| `/stat/measure` | read | buffer fill, stage count, per-stage size |
| `/stat/crit` | read | the interrupt-masking budget measured at runtime |
| `/dev/log` | read/write | 8-line ring; **drops the oldest rather than ever blocking** |
| `/dev/led` | write | on-board blue LED on PG7; `on` / `off` / `toggle` |

### Commands

```
ls
cat <path>
echo <text> > <path>
help
```

A read handler returns `Result<(), ()>`; `Err` prints `(not ready)` rather than pretending success.
That is the non-blocking rule from the plan, expressed in the type signature instead of a comment.

### Where the CLI runs

In the **LOAD task** — the lowest priority — deliberately. A console is untrusted load: it is slow,
it may block on I/O, and it must not be able to touch the real-time plane.

Measured: while 15 scripted commands executed back to back, `overruns = 0` and
`tick_missed = 0`. The real-time task never noticed.

### Console is an abstraction, and both back ends are now verified

`cli::out()` writes through a `Console` enum with two back ends: `Rtt` and `Uart`.

An earlier version of this document claimed the UART path was unverified "because there is no
USB-TTL adapter here". **That was wrong.** The DAPLink-style probe on this desk has an integrated
USB-to-serial bridge, and it shows up in Windows as:

```
USB 串行设备 (COM16)   USB\VID_0D28&PID_0204&MI_00
```

Same VID/PID as the debug probe (`0d28:0204`), different interface. So a serial port was available
the whole time; the assumption was never checked. With PA9/PA10 wired, `COM16` is USART1 at
115200 8N1.

Two defects surfaced only once it was exercised for real:

1. **`uart::init()` was never called.** The driver was written and wired into the module tree, but
   nothing invoked it, so the port stayed silent. A whole device tree was "verified" through RTT
   while the actual console path had never been initialised.
2. **`
` was sent without ``.** `cli::UartWriter` called `uart::write_byte` directly, bypassing
   the CRLF expansion in `uart::Writer`. Terminals then advance without returning to column 0.

Byte-level check after the fixes (`help`, 292 bytes):

```
68 65 6c 70 0d 0a ... e8 af b4 e6 98 8e 0d 0a 0d 0a e6 89 80 ...
h  e  l  p  CR LF     说   明              CR LF CR LF  所
```

UTF-8 and CRLF both correct, including the blank line between paragraphs.

```
zaygo> ls
r-  /stat/sched          实时面运行快照
r-  /stat/measure        采样缓冲状态
r-  /stat/crit           中断屏蔽预算实测值
rw  /dev/log             环形日志；满了丢最旧的，永不阻塞
-w  /dev/led             板载蓝灯 PG7；写 on/off/toggle
zaygo> cat /dev/log
(not ready)
zaygo> echo first line > /dev/log
ok, buffered
zaygo> cat /dev/log
 1: first line
zaygo> cat /nope
err: 没有这个设备: /nope
zaygo> echo x > /stat/sched
err: /stat/sched 是只读的
zaygo> echo nonsense > /dev/led
err: 只接受 on / off / toggle
```

The LED write was checked against hardware: after `on` then `toggle`, `GPIOG.ODR` bit 7 reads 0.
The command actually moved a pin.

### What is deliberately missing

`/dev/actuator/*/ctl` and `/ctl/sched/admit` from the plan are **not implemented**. The rule that
"every write is a submitted request executed by the real-time plane, with an asynchronous ack"
is the whole point of P1, and faking it with a direct `ODR` write would be worse than not having it.
`/dev/led` is honest about what it is: a status indicator, not an actuator with a control loop.
