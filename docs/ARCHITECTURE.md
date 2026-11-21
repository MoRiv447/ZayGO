# Architecture Supplement: Display Plane, Scenario Templates, and Tiered Extensibility

> English version. [中文版](ARCHITECTURE.zh-CN.md)

> Companion document: `POSITIONING.md` (positioning rationale). This document records the design constraints settled in the third round of discussion.
> Status: the design direction is decided, the details await implementation validation. All conclusions are based on public information retrieved on 2026-11-19.

---

## Zero: Two Corrections to the Previous Round's Conclusions

### Correction 1: The STM32H7 does have an MPU (the previous round said it did not — that was wrong)

Verbatim from the official ST STM32H723 page:

> "STM32H723xE/G devices support a full set of DSP instructions and **a memory protection unit (MPU)**
> to enhance application security."

The Cortex-M7 MPU is an **optional** feature; ST implements it in the H7 family (mainstream parts such as the H723/H735/H743).
MPU capabilities: 8 main regions + 8 sub-regions per region, privileged/unprivileged access rights, Normal/Device/Strongly Ordered memory type attributes.

**Impact**: the partition isolation scheme **does not require moving to an M33/M55**; the STM32H723 on hand can serve as the target platform.

### Correction 2: The H723 has plenty of memory, the framebuffer is not the bottleneck

STM32H723xE/G: 550MHz Cortex-M7, 1MB Flash, **564KB RAM**
(192KB shareable between ITCM/AXI + 64KB dedicated ITCM + 128KB dedicated AXI + 128KB DTCM + 48KB AHB + 4KB backup).

For reference, ST's own positioning of the STM32H7A3: *"1.4 MB of SRAM can store an entire frame buffer"* — graphics is the main battlefield ST itself assigns to the H7.

---

## One: Display Plane Design — L8 + CLUT

### 1.1 Hardware-Native Support (verified)

Verbatim from the STM32H723 **Chrom-ART accelerator (DMA2D)** manual:

> "All the classical color coding schemes are supported from 4-bit up to 32-bit per pixel
> **with indexed or direct color mode**... **The DMA2D has its own dedicated memories for CLUTs
> (color look-up tables)**."

**L8 + CLUT maps exactly onto the DMA2D native operating mode**; it is not a software-emulated palette.

### 1.2 Framebuffer Size Comparison

| Format | 320×240 | 480×272 | 800×480 | Notes |
|---|---|---|---|---|
| RGB565 (16bpp) | 150KB | 255KB | 750KB | Double buffering doubles it |
| RGB888 (24bpp) | 225KB | 382KB | 1125KB | Does not fit in the H723 |
| **L8 indexed (8bpp)** | **75KB** | **127KB** | **375KB** | With a 256-entry CLUT (512B) |

L8 saves 50% against RGB565 and 66.7% against RGB888. On 564KB RAM, L8 can do double buffering (150KB total),
while RGB565 is tight even for single buffering.

### 1.3 Three Unique Capabilities Brought by L8

1. **Zero-cost skinning**: rewrite the 256×2-byte CLUT and the whole UI palette switches instantly.
   RGB565 cannot do this (the color is encoded directly into the pixels).
2. **Inversion / night mode**: invert the CLUT, no redraw needed.
3. **The framebuffer is hashable and diffable**: every byte is an index rather than a color,
   so "where did this frame change from the previous one" is a byte-level comparison, with no image processing required.

Point 3 fits naturally with this project's forensics / evidence chain methodology.

### 1.4 Display Plane Positioning: Table Lookup Rather Than Rendering

L8 + CLUT reduces "rendering" to "table lookup":

```
sensor reading ──→ mapped to index i (0..255) ──→ CLUT[i] ──→ screen colour
```

- Complexity drops from O(layout algorithm) to O(table lookup)
- The mapping rule can be verified ahead of time (a pure function, amenable to property-based testing)
- The index space is a **finite set of semantic slots** (temperature bands, status icons, progress-bar segments),
  which is naturally declarable as part of the template schema

### 1.5 Cache Coherency: The Hidden Cost of the L8 Approach (must be handled)

The Cortex-M7 has a 32KB D-Cache (write-back by default). DMA2D and LTDC **read physical memory directly** and cannot see dirty data in the cache.

Verbatim from ST application note DM00272913:

> "If the CPU writes something to an area that has a write-back cache attribute, the write result is not
> seen on the SRAM as the access is buffered, and then if the DMA reads the same memory area to perform
> a data transfer, the values read do not match the intended data."

**Known pitfall instance**: on the ST community forum there are TouchGFX users who restricted cache maintenance operations to the framebuffer range,
and still hit **ghosting (ghost images) and HardFault** — because the framebuffer spans two regions, AXI SRAM (write-through) and SDRAM (write-back),
and under a mixed policy `SCB_CleanInvalidateDCache_by_Addr` is not enough to guarantee visibility.

**Four official solutions**:

| Option | Approach | Cost |
|---|---|---|
| A | Use the MPU to mark the framebuffer **Non-Cacheable** | Simple; the performance loss is acceptable; recommended by the community |
| B | Keep Write-Back and explicitly `SCB_CleanDCache_by_Addr` after each write | Highest performance; **miss one spot and the screen is corrupted** |
| C | Switch to Write-Through | Suitable for TX only; errata exist on H74x/H75x r1p1 and earlier |
| D | Place the whole framebuffer in a contiguous Non-Cacheable region | Alignment and contiguity requirements must be met |

**This project's decision: A**. Reason: carve the framebuffer out as its own MPU region and mark it Non-Cacheable,
so one display bug cannot pollute other memory and one memory bug cannot make DMA2D read torn data.
This is consistent with the "reliability" goal — trading a little performance for a deterministic failure boundary.

### 1.6 Two Hard Hardware Constraints (you only know after stepping on them)

1. **DMA1/DMA2 cannot reach DTCM / ITCM** (0x20000000).
   DMA1/DMA2 live in the D2 domain; only MDMA (D1 domain) can access TCM.
   ⇒ **The framebuffer must go in AXI SRAM / SRAM1-3 / SDRAM, not DTCM.**
2. **The LTDC layer framebuffer start address must be 16-byte aligned**; 32-byte alignment is recommended
   (the cache line is 32 bytes, and 32-byte alignment avoids the granularity trap in cache maintenance operations).

### 1.7 Chinese Fonts: The Real Cost Item

The framebuffer is not the bottleneck, **the fonts are**. Measured data:

| Option | Size | Rendering Overhead |
|---|---|---|
| Full CJK TTF | 10MB+ | Requires real-time rasterization with FreeType, not feasible |
| Full GB2312 16×16 bitmap | 216KB | memcpy + bit masking, near zero |
| Full 16px Chinese font set (LVGL) | 2.8MB | — |
| LVGL 16pt ASCII, 95 characters | 12KB | — |
| LVGL Chinese basic block, 20902 characters | **2.1MB** | — |
| 20px / 500-Chinese-character subset on ESP32-C3 | **142KB** | 60FPS maintained |
| Pulse-oximeter measurement: 32 Chinese characters + 15 ASCII | **≈1KB** | Covers 100% of the UI text |

**Decision: subset the fonts at compile time against the strings the scenario template actually uses, generate a bitmap font library, and burn it into the image.**
Generate it offline with `pyftsubset` + `fonttools`, emitting a static array.

This is an extension of the same logic as "the application set is fixed": **the scenario template declares what text to display, and compile time trims the font library accordingly.**
If this chain does not close, the whole approach leaks in Chinese-language scenarios.

### 1.8 The Display Plane Must Never Block the Real-Time Plane

- Framebuffer updates go through DMA2D, no CPU time spent on per-pixel copying
- The display task's own priority is below the real-time plane, and its bandwidth is capped
- Display failures (corrupted screen, missing font library) must not propagate to the real-time plane: MPU partition isolation is the backstop

---

## Two: Scenario Templates — the True Source of Generality

### 2.1 Core Insight

"If installing software at runtime is not allowed" taken to the extreme turns the project into a closed vertical product, and generality drops to zero.
**Compile-time assembly is the antidote**:

> What the template produces is still one firmware image, one hashable and signable artifact,
> a finite, exhaustible set of execution paths. **Runtime immutability is fully preserved,
> but the same kernel can serve multiple scenarios.**

This is not a new invention, it is a pattern validated at scale: ESPHome also "compiles into the firmware" rather than "installing onto the device",
yet serves millions of different scenarios.

### 2.2 Difference from the NuttX / Zephyr Configuration Systems

| Design Choice | NuttX | Zephyr | **This Project** |
|---|---|---|---|
| Configuration Vehicle | Kconfig menus (combinatorial explosion) | Kconfig + Devicetree (**two sources of truth**) | **A single declarative scenario file** |
| Number of Sources of Truth | One | Two, must be kept in sync | **One** |
| Combination Scale | 1500+ templates | Tens of thousands of configuration items | **Dozens of scenario templates** |
| Artifact | The configured firmware | The configured firmware | Firmware + **schedulability report** + SBOM |
| Main Weakness (public assessments) | Heavy configuration burden, steep learning curve | Two sources of truth easily drift apart | — |

Public material lists NuttX's 1500+ templates as its main weakness:
*"its hundreds of configuration options make maintenance substantially more complicated."*
This project must avoid that pitfall: **dozens of scenario-level templates, not thousands of configuration options**.

### 2.3 Shape of a Scenario Template (illustrative)

```yaml
# scenes/guard-panel.zay
scene: guard-panel
display:
  width: 320
  height: 240
  format: l8-clut
  palette: industrial-dark
  fonts:                       # compile-time subsetting input
    - { family: noto-sans-cjk, size: 16, chars: "温度 压力 报警 正常 故障 状态 时间" }
realtime:
  - { task: imu-sample,  period: 5ms,  deadline: 1ms, budget: 15% }
  - { task: loop-ctrl,   period: 10ms, deadline: 2ms, budget: 10% }
management:
  - { task: http-server, bandwidth: 20% }
  - { task: shell }
extensions:
  wasm_pool: { max_modules: 4, fuel_each: 100000, memory_each: 64KB }
drivers: [ imu, can, display-st7789, w25q128 ]
```

### 2.4 One File, Three Identities

This is the most valuable design point in this project:

1. **Compile-time input** — determines which modules end up in the image
2. **Basis for admission control** — each task's `budget` is explicitly declared and can take part in schedulability analysis
3. **Input to the schedulability report** — aligned with the analysis tool's output

"Generality" and "predictability" are tied to **the same file**, not to two unrelated mechanisms.

### 2.5 Incidental Benefits

- Scenario → image → SBOM → hash, one chain.
  The SBOM format can borrow from the SPDX 3.0 support newly added in Zephyr 4.5.
- One kernel, multiple scenarios, each with its own schedulability report and timing evidence.
- The template itself is a declarative structure ⇒ **the schema can be formally verified**,
  and this can proceed in parallel with the Verus verification of the kernel invariants.

---

## Three: Tiered Extensibility — Generality Not Cut to Zero

"If installing software is not allowed" taken to the extreme turns the project from a "platform" into a "product", and the ecosystem dies.
The lesson of Plan 9 applies here — Drew DeVault's assessment:

> "Plan 9 failed, in a sense, because Unix was simply too big and too entrenched at the time."

So tiered extensibility is retained, but **native code loading must be forbidden**:

| Tier | Who Can Write It | Can It Be Loaded Onto the Device | Predictability Guarantee |
|---|---|---|---|
| L0 built-in applications | The project itself only | Compiled into the image | Full guarantee (exhaustible) |
| L1 declarative scenario configuration | The user | Compiled into the image | High (executes no arbitrary code) |
| L2 Wasm plugins | Third parties | Loaded at runtime | **Bounded** (fuel + memory page, two upper bounds) |
| L3 native code | Anyone | **Forbidden** | None |

### 3.1 Why L2 Is Feasible: the Key Properties of wasmi (verified)

| Property | Description |
|---|---|
| Language | Rust (same language as the kernel, no cross-language boundary) |
| Deployment | `no_std` is available, suits MCU |
| **fuel metering** | Built in, bounding for the host how much execution a module may consume |
| Memory | Linear memory is allocated by page ⇒ the memory footprint has a hard upper bound |
| Execution Model | Interpreted execution, **no JIT**, no W^X problem |
| Determinism | Behavior is reproducible, suitable for auditing |
| Compliance | 100% WebAssembly spec testsuite |
| Security Audit | Two external audits: Runtime Verification Inc. (0.36–0.38, commissioned by the Stellar Foundation) and SRLabs (0.31, commissioned by Parity Technologies); the report PDFs have been committed to the repository |
| Version | 2.0.0 released on 2026-09-01; 2.0 binds fuel metering to the input Wasm rather than internal bytecode, making module cost stable across versions |
| Known Limitations | function references / GC / threads / exception handling are still tracking issues; modules containing these features cannot be loaded |
| Ecosystem | 830 crates depend on it directly/indirectly; about 1.78 million downloads per month |

**Why this combination matters**: the two upper bounds — fuel and memory pages — are exactly the inputs that WCET analysis and RAM analysis need.
⇒ "General" and "bounded and predictable" can hold simultaneously at the L2 tier.

### 3.2 Language and Toolchain Consistency

Wasm is a first-class citizen of Rust (the `wasm32-unknown-unknown` target); the kernel, the plugins, and the host tooling all live in the same language ecosystem.

---

## Four: Scenario Template × Four Planes — the Final Architecture

```
┌─ Scenario template (only build-time entry) ─┐   ┌─ Single firmware image ─┐
│ Declarative scenario file                    │──▶│ Hashable, signable       │
│ Every task declares period / deadline / budget│   │ Finite, enumerable paths │
│ Font subsetting input                        │   │ Schedulability report    │
└──────────────────────────────────────────────┘   │ SBOM                     │
                                                   └───────────┬──────────────┘
┌─ ZayGO runtime · MPU partitioned ─────────────────────────┴─────┐
│ ┌─────────────────────────────────────────────────────────────┐ │
│ │ Real-time plane  exclusive budget, zero dynamic alloc,       │ │
│ │                  never waits holding a lock                  │ │
│ ├─────────────────────────────────────────────────────────────┤ │
│ │ Display controller  L8 unified framebuffer + 256-entry CLUT  │ │  ← third revision
│ │                     per layer                              │ │
│ │                     backends: LTDC hardware scan-out / SPI  │ │
│ │                     framebuffer marked Non-Cacheable        │ │
│ ├─────────────────────────────────────────────────────────────┤ │
│ │ Management plane HTTP/WS data out, bandwidth capped,        │ │
│ │                  always preemptible, carries touch events   │ │
│ ├─────────────────────────────────────────────────────────────┤ │
│ │ Plugin plane     Wasm sandbox, fuel + memory-page bounds    │ │
│ └─────────────────────────────────────────────────────────────┘ │
│ ┌─────────────────────────────────────────────────────────────┐ │
│ │ Unified device API  namespace addressing · non-blocking      │ │
│ │                       message passing                       │ │
│ └─────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────┘
```

### 4.1 Summary of Plane Responsibilities and Constraints

| Plane | Priority | Memory Attributes | Bandwidth Cap | Can It Touch Hardware |
|---|---|---|---|---|
| real-time plane | Highest | TCM / exclusive | Exclusive budget | **Direct read/write, the only one allowed** |
| display controller | High but can yield | Dedicated Non-Cacheable region | Near zero for the LTDC backend; limited for the SPI backend | Read-only snapshot, does not change state |
| management plane | Low | Ordinary cacheable | Hard cap (e.g. 20%) | Only submits requests via the device API |
| plugin plane | Lowest | Sandboxed linear memory | fuel upper bound | Only submits requests via the device API |

**Key rule: every write is a "submit a request", not a "change state directly"**,
and all state changes are performed and acknowledged by the real-time plane. This is the mechanism that completely seals off the threat of "Web/plugins are unpredictable".

### 4.1b Display Controller Layer: Dual-Backend Design (added in the third round)

The L8 framebuffer is the **uniform intermediate format that is invariant across backends**. The upper layer's rendering always writes L8 indices only
and never perceives backend differences; swapping the CLUT is swapping the skin, and swapping the backend changes not a single line of rendering code.

```
┌─ Display controller layer (backend chosen by the scenario template) ─┐
┌────────────────────────┐ ┌──────────────────────┐ │
│ LTDC backend (primary)  │ │ SPI backend (cost)   │ │
│ parallel RGB panel,     │ │ panel has no CLUT, so │ │
│ hardware scan-out,      │ │ software must expand │ │
│ zero CPU involvement,   │ │ DMA2D does indexed   │ │
│ native L8 + 256-entry   │ │ to RGB565; bus       │ │
│ CLUT per layer,         │ │ contention becomes   │ │
│ 800x480 L8 double       │ │ the interference     │ │
│ buffered = 768KB        │ │ source; refresh by   │ │
│                        │ │ dirty rectangles     │ │
└────────────────────────┘ └──────────────────────┘ │
├──────────────────────────────────────────────────────┤
│ L8 unified framebuffer · Non-Cacheable · 32-byte aligned · never DTCM │
└──────────────────────────────────────────────────────┘
```

#### LTDC Natively Supports L8 + CLUT (key forensics conclusion)

Verbatim from the STM32 LTDC manual (applies to H7 / N6):

> "**Color look-up table (CLUT) up to 256 colors (256x24-bit) per layer**"
>
> The input pixel formats include "**L8 (8-bit luminance or CLUT)**", AL44, AL88

Chinese-language sources put it more bluntly:

> "L8 (8-bit luminance or CLUT) is an 8-bit color format that is in fact only an 8-bit index value, ranging 0–255,
> and the specific color value of each index is stored in the color look-up table CLUT."

**This is more direct than the DMA2D approach**: the LTDC reads indices straight out of the L8 framebuffer and expands them with a **hardware CLUT**
into RGB888/RGB565 for the panel, with zero CPU involvement and **no need for DMA2D to participate in the transfer**.
The CLUT is written layer by layer via `LTDC_LxCLUTWR`, each layer independent.

#### SPI Displays (ST7789) Have No CLUT — a Hard Limit

The ST7789's COLMOD (0x3A) supports only 12/16/18 bits (RGB444/RGB565/RGB666),
with **no indexed mode and no CLUT register**.

⇒ On H723 + ST7789, L8 cannot drive the panel directly; it must first be expanded to RGB565:

- DMA2D can do the indexed → RGB565 pixel format conversion (hardware support)
- **SPI bandwidth becomes the bottleneck**: 240×320×2 = 153KB/frame; SPI at 40MHz is about 5MB/s
  ⇒ a full-screen refresh takes about 30ms (about a 30fps ceiling), and it requires CPU/DMA participation throughout
- The ST7789's GRAM + CASET/RASET window mechanism can refresh only the dirty rectangle to mitigate this

**Conclusion: the ST7789 is usable, but it belongs to the "SPI display + software rendering" route and is not the L8+CLUT validation platform.**

### 4.1c Division of Roles Between the Two Boards

| | H723 + ST7789 | N647 + 4.3" RGB LCD |
|---|---|---|
| Bus | SPI (CPU/DMA pushes pixels) | **Parallel RGB (LTDC scan-out)** |
| L8 + CLUT | ✗ No CLUT on the panel, software expansion needed | ✓ **LTDC hardware CLUT natively supported** |
| CPU Cost of Screen Refresh | High (limited by SPI bandwidth) | **Zero** |
| Touch | ✗ None | ✓ GT911, I2C + external interrupt, 5-point capacitive |
| Resolution | 240×320 | 800×480 (or 480×272) |
| Impact on Real-Time Performance | **Yes** (bus contention) | **Almost none** (pure DMA) |
| Memory Budget | L8 double buffering 150KB / 564KB RAM | L8 double buffering **768KB** / 4.2MB SRAM + 32MB HyperRAM |
| Positioning | **Cost-sensitive tier** | **Main flagship tier** |

One kernel, two display backends, selected by the scenario template — this is exactly where the scenario template system earns its keep.

#### Additional Graphics Hardware on the N6

- **Chrom-GRC (GFXMMU)**: "Fully programmable display shape to physically store only the visible pixel",
  4 virtual buffers, each 3072/4096 bytes per line × 1024 lines, mappable to arbitrary system memory;
  saves memory for non-rectangular displays such as round panels.
- ST's own words: 4.2MB RAM "could store a double frame buffer for a 1280 x 800 display".
- **Neo-Chrom GPU2D**: supported formats include **L8**, CLUT (256 entries), ARGB8888, RGB565, RGB888,
  A8/A4, AI44, AI88; command-list based DMA with extremely low CPU overhead.

### 4.1d Touch: A Newly Introduced Real-Time Risk Dimension

The GT911 is **I2C + external interrupt PEN** (the I2C address is set by configuration resistors or OTP; 0x14 / 0x5D are common).

Touch is **external asynchronous input**; if handled badly it hits the real-time plane directly. Mandatory rules:

```
touch interrupt (highest priority, does the minimum)
  -> read the GT911 registers (I2C, short transaction)
  -> post an event message to the management plane
  -> the management plane updates the UI state
  -> render into the L8 framebuffer
  -> the LTDC scans it out in hardware
```

**Hard rules**:

1. **Drawing in the ISR is absolutely forbidden; reading sensors in the ISR is absolutely forbidden**.
   The interrupt does exactly two things: "read registers + submit a message".
2. **The touch I2C bus is exclusive**. If real-time sensors hang off the same I2C bus,
   the management plane must not occupy that bus — bus ownership must be declared explicitly in the scenario template.
3. **Touch events only change UI state**, never device state directly.
   Any device state change must still be submitted through the device API to the real-time plane for execution.

> Capacitive touch panels **need no calibration** (per the vendor's own documentation from ALIENTEK), which saves one step in the process.


### 4.2 Device API Mapping (Plan 9 Style, Changing the Blocking Semantics)

The core of Plan 9 is not "everything is a file", but **a private namespace per process** +
**controlling everything through text files** + **9P bridging local and remote**.
Its concrete approach can be borrowed directly:

- To open a TCP connection: open `/net/tcp/clone` to get an id → write
  `connect 10.0.0.1!80` to `/net/tcp/<id>/ctl` → read/write `/net/tcp/<id>/data`
- **No socket system calls**; dial() is just a few file operations and can be implemented in a shell script
- Device control goes through writing text to `ctl` files instead of binary ioctl ⇒ `echo` / `cat` can drive the hardware

| Real-World Object | Plan 9 Style Interface | Real-Time Note |
|---|---|---|
| Sensor | `/dev/sensor/<id>/value` (read = sampling) | Sampling goes via DMA/interrupts, reads do not block |
| Actuator | `/dev/actuator/<id>/ctl` (write a text command) | The write is queued immediately and consumed by the real-time plane |
| Configuration | `/etc/<key>` | Static read-only partition |
| Log | `/dev/log` | Ring buffer, drops the oldest instead of blocking |
| Status | `/stat/<task>` | Read-only snapshot |
| Task Quota | `/ctl/sched/admit` | Admission control entry point |

**Key difference from Plan 9**: Plan 9's blocking file semantics do not suit hard real-time.
This project switches to **non-blocking message passing**, but keeps the unity of "namespace + text control + one protocol".

---

## Five: How the Four-Dimensional Goals Hold Simultaneously

The previous round pointed out that "general + real-time + reliable + predictable" are mutually exclusive. This round's combination of four constraints gives the solution:

| Goal | Which Constraint Achieves It |
|---|---|
| Generality | L0-L2 tiered extensibility (templates + Wasm plugins), L3 forbidden |
| Predictability | Fixed application set → WCET computable; fuel/memory upper bounds → bounded plugins; admission control → reject once over capacity |
| Reliability | MPU partition isolation; Non-Cacheable framebuffer; Rust memory safety; a single hashable artifact |
| Real-Time Performance | Bandwidth cap; non-blocking device API; write operations changed to request submission |

**Key point: it does not rest on a single mechanism, it rests on all four stacked together.**
seL4 has verifiability and partitioning but no path to generality;
Zephyr has generality but explicitly gives up timing guarantees;
this project's combination has no precedent on the MCU side.

---

## Six: Decided and Open

### 6.1 Decided (First Round)

- The device **has a screen** but does not render HTML; display goes through **L8 indexing + a 256-entry CLUT**
- The L8 framebuffer is **Non-Cacheable** (MPU configuration), no manual cache maintenance
- The framebuffer sits in **AXI SRAM**, **not in DTCM** (DMA2D/LTDC cannot reach it)
- LTDC layer addresses are **32-byte aligned**
- Chinese fonts are **subsetted at compile time** into a bitmap font library, trimmed to the strings the scenario template actually uses
- Generality comes from the **L0-L2 tiers**, and **L3 native code loading is forbidden**
- The scenario template is the **single compile-time entry point**, one declarative file

### 6.2 Decided (Third Round: Four Decisions + Hardware Platform)

| Decision | Conclusion | Reason Summary |
|---|---|---|
| L2 Wasm plugin tier | **Do it** | wasmi is ready-made (Rust / no_std / fuel + memory page, two upper bounds / two external audits); it keeps generality without breaking the "fixed application set" |
| Device API | **Plan 9 style** | Namespace + ctl text files + a private namespace per process; blocking semantics changed to non-blocking message passing to fit hard real-time |
| Admission criteria | **Hybrid** | The real-time plane uses static WCET (sound); the management plane uses measurement-based MBTA + EVT (light) |
| Scope of formal verification | **Verify only the scheduler's core invariants** | Follow the UCSB retrofit path (adding verification surgically to an existing system), do not chase a whole-kernel proof |

**Hardware platform (revised in the third round)**:

- **Main platform: STM32N647 + 4.3" RGB LCD (ATK-MD0430R)**
  - Cortex-M55 800MHz, 4.2MB built-in SRAM, 32MB HyperRAM, 32MB NOR Flash
  - LTDC parallel RGB direct drive, **native L8 + a 256-entry CLUT per layer**
  - GT911 capacitive touch (I2C + external interrupt, 5 points, **no calibration needed**)
  - 800×480 L8 double buffering 768KB, it fits in the built-in SRAM
  - **Zero CPU participation in screen refresh ⇒ near-zero impact on real-time performance**
- **Cost-tier platform: STM32H723 + ST7789**
  - 550MHz, 564KB RAM, has an MPU (**correcting the first round's misjudgment**)
  - The SPI panel has no CLUT ⇒ it needs software expansion to RGB565, and **bus contention becomes a source of interference**
  - Positioned as the low-tier scenario template platform, not the L8+CLUT validation platform
- The two boards cover **two display backends**, selected by the scenario template — this is exactly where the template system earns its keep

### 6.3 Still Open (engineering detail level, does not block startup)

1. **The concrete list of invariants to prove in Verus**: the recommendation is to start with only three
   (① register state is intact after a context switch; ② priority inheritance has no deadlock; ③ admission control does not exceed capacity).
2. **Schedulability analysis toolchain**: build a response-time analyzer in-house, or port Linux's CBS/`SCHED_DEADLINE` semantics.
3. **Resource declaration format for the Wasm host ABI**: how the fuel and memory page upper bounds are expressed in the scenario template.
4. **The precise semantics of the non-blocking device API**: does `/dev/xxx/read` return "data not ready" or "partial data + wait for more",
   and it must be unified with Plan 9's `ctl` text style.
5. **Whether the H723 is kept as an officially supported second platform**, or serves only as an experimental testbed.

---

## Seven: Sources

| Conclusion | Source |
|---|---|
| The H723 has an MPU, 564KB RAM, 550MHz | ST's official STM32H723VG product page and the DS13313 datasheet |
| DMA2D supports indexed mode and dedicated CLUT storage | STM32H723/H735 datasheet, section 3.13 Chrom-ART Accelerator |
| **The LTDC natively supports L8 + a 256×24bit CLUT per layer** | STM32 LTDC manual (wiki.st.com LTDC feature page); EmbedFire / ALIENTEK Chinese-language LTDC material |
| The D-Cache and DMA coherency problem and its four solutions | ST application note DM00272913《Level 1 cache on STM32F7/H7 series》 |
| DMA cannot reach DTCM, mixed cache policies cause ghosting | ST community《DMA is not working on STM32H7 devices》《Granular D-Cache maintenance operations cause artefacts or hardfaults》 |
| LTDC alignment requirements and cache configuration practice | STM32 community and lpqz.cn《Driving an RGB display with STM32H743: hands-on with LTDC and DMA2D hardware acceleration》 |
| **The ST7789 has no indexed mode and no CLUT** | ST7789V manual COLMOD(0x3A) definition; several driver implementation cases (only 0x05/0x06/12bit) |
| **The N647 board carries a 4.3-inch RGB LCD + GT911** | The official ALIENTEK N647 wiki (touchscreen experiment chapter), the ATK-MD0430R module repository README |
| N6's Chrom-GRC / Neo-Chrom / 4.2MB RAM | STM32N647xx datasheet sections 3.12/3.13/3.30; the ST official blog |
| Measured Chinese font sizes | LVGL / NXP GUI Guider / U8g2 field cases (16px full set 2.8MB, 20902 characters 2.1MB, 500 Chinese characters 142KB, 32 Chinese characters about 1KB) |
| wasmi capabilities and audits | lib.rs/crates/wasmi, the hysenlabs project page, the wasmi GitHub README (audit report PDFs committed to the repository) |
| Plan 9 private namespaces and the 9P design | The vectree.io Plan 9 entry, drewdevault.com《In praise of Plan 9》, 9p.io architecture material |
| NuttX configuration burden and minimum size | The everythingxiaomi.com NuttX review, the hivebook.wiki Apache NuttX entry |
| ESPHome's compile-time model and scale | The official esphome.io documentation |
| Zephyr 4.5 SPDX 3.0 SBOM | The zephyrproject.org 4.5 release announcement (2026-10-28) |
