# Project Positioning Analysis: General-Purpose Real-Time Kernel (project name: ZayGO)

> English version. [中文版](POSITIONING.zh-CN.md)

> Status: a positioning argument made before the project is kicked off. All conclusions are based on public information retrieved on 2026-11-19.
> Method: first take forensics on the actual capability boundaries of three reference projects, then locate the position nobody occupies, and finally list the optional routes and their costs.
> **This document does not make the choice for the decision-maker; it only lays out the options and the trade-offs clearly.**

---

## 1. Problem Statement

Goal: on high-performance MCUs (Cortex-M7/M33/M55, RISC-V, etc.), provide an operating system kernel that simultaneously offers **generality, real-time performance, reliability, and predictability**,
in a form similar to "a combination of GPOS and an RTOS".

Two questions must be answered first:

1. Where is the **real gap** in this direction? What cannot the existing projects (Zephyr / seL4 / NuttX) do?
2. How should the project be **positioned**, so that it is differentiated without degenerating into "yet another RTOS"?

---

## 2. Forensics on the Reference Projects (as of 2026-11-19)

### 2.1 Capability Boundary Comparison

| Dimension | Zephyr 4.5 | seL4 16.0 | NuttX 13.1 |
|---|---|---|---|
| Positioning orientation | Breadth first | Correctness first | POSIX completeness first |
| Ecosystem scale | 1000+ boards, 11 architectures (TriCore / Hexagon added in 2026), 1200+ contributors in a single release | Few platforms, but a full toolchain: Microkit / CAmkES / capDL / rust-sel4 | 300+ boards, 1500+ configuration templates, 15+ CPU architectures |
| Process model | Threads + optional userspace (MPU) | Microkernel + capability system + MCS scheduling | POSIX processes (flat or protected), fork / mmap / socket |
| Networking / storage | Full stack (lwIP); 4.5 adds QUIC / HTTP3 / SSH / IPv4 forwarding NAT / WireGuard | Depends on user-space components (NetBSD, etc.) | BSD sockets + optional littlefs / ROMFS / VFAT / NFS |
| Formal verification | None | Kernel functional correctness + integrity + confidentiality (fully proved on AArch64; confidentiality completed 2026-08) | None |
| Memory safety | None (Zephyr uses C) | Yes (the kernel is C, but it is proved, plus Rust support 5.0) | None (Apache-2.0, C) |
| Minimum footprint | Depends on configuration, far larger than an RTOS | Extremely small kernel (microkernel) | ~100 KB Flash / 16 KB RAM as a starting point (minimum configuration ~32 KB) |
| Deployment evidence | 10M+ devices; Bosch EDC17 engine ECU (TriCore) | High-assurance embedded (base stations, flight control, safety-critical) | PX4 drones, SONY TizenRT, Xiaomi Vela |
| Community governance | Linux Foundation, vendor membership dominated | seL4 Foundation (led by UNSW; ANU joined 2026-09) | Apache top-level project, 2–10 core people |

### 2.2 Key Evidence Excerpts

**Zephyr officially refuses to make timing guarantees.** Verbatim from the doc comment of `k_thread_deadline_set()` in `include/kernel.h`:

> "Despite the API naming, the scheduler makes no guarantees the thread WILL be scheduled within that deadline,
> nor does it take extra metadata (like e.g. the 'runtime' and 'period' parameters in Linux `sched_setattr()`)
> that allows the kernel to validate the scheduling for achievability. **Such features could be implemented
> above this call, which is simply input to the priority selection logic.**"

Reading: Zephyr's `CONFIG_SCHED_DEADLINE` is only a **tiebreak for ordering within the same priority level**, not a real EDF scheduler;
it **does not take runtime/period metadata**, therefore **the kernel itself cannot do admission control or schedulability determination**.
This is Zephyr explicitly choosing generality in the "generality ↔ predictability" trade-off.

**seL4's proofs do not cover applications and drivers.** What seL4 proves is the kernel's own functional correctness, integrity, and confidentiality.
On WCET, seL4 and CertiKOS are academic precedents, but **kernel WCET excludes drivers and applications**;
application-level timing still has to be analyzed separately (usually relying on commercial tools such as aiT / RapiTime).

**NuttX explicitly does not pursue Linux-class breadth.** Official stance: small footprint first, Linux-class breadth is not the goal.
The cost shows up as follows: ~32 KB RAM for the minimum configuration, and 1500+ configuration templates bring a significant configuration learning cost.

**Zephyr's statements about timing guarantees are directly challenged by academia.** Verbatim from the 2026 paper by UC Davis,
"The Time is Right: Retrofitting Formal Verification onto Timers in an Operating System":

> "The Zephyr RTOS, a popular open source RTOS, **provides no guarantees at all on timing** other than
> experimental benchmarks. This level of assurance is not enough for critical systems where failure can cost
> millions of dollars. The seL4 and CertiKOS operating systems formally verify their worst case execution time (WCET)."

---

## 3. The Real Gap: What All Three Lack

The three are not homogeneous competitors, but **three mutually exclusive trade-off strategies** (generality / verifiability / POSIX completeness).
Therefore "building yet another general-purpose + real-time hybrid" is **already crowded at the kernel design level**, and the pure-kernel route lacks differentiation.

Yet all three share one structural gap:

### Gap 1: No "reconciliation mechanism" between generality and predictability

| Mechanism | What it does | Representative implementation | Who has it | Who does not |
|---|---|---|---|---|
| **Admission control** | Runs schedulability analysis when a task applies; rejects over-capacity requests instead of overrunning afterwards | CBS / SCHED_DEADLINE | Linux (mature on the AP side) | All three MCU RTOSes have none |
| **Static partitioning** | Uses MPU / domains to physically isolate general-purpose code from hard real-time code, each analyzed independently | Bao hypervisor, seL4 domains | seL4 (rather heavy) | Zephyr / NuttX have no partitioning abstraction |
| **Static timing analysis** | Computes a WCET upper bound + response time analysis, proving no deadline miss | aiT / Bound-T / RapiTime | Commercial tools ($20k–$100k) | None of the three integrates any |

> Key point: Zephyr's comment states outright "Such features could be implemented above this call" —
> **it leaves the door open; nobody has walked through it.**

### Gap 2: Nobody occupies the middle ground between "verifiable" and "good enough" on an MCU

- The seL4 route is verifiable, but the ecosystem is thin, the entry barrier is high, and the learning curve is steep (capability / capDL / Microkit).
- The NuttX route is good enough, but there is no formal guarantee and no functional safety certification.
- The Zephyr route is general-purpose, but it **explicitly abandons timing guarantees** (see the 2.2 quote).

**The middle ground exists**: a system that is "a Rust kernel + partial formal verification + POSIX-style API + actually runnable on an MCU".

The preconditions on the Rust side matured in 2026:

- **Verus** (CMU + Amazon) is open source and verifies the functional correctness of Rust functions, not merely memory safety;
  it has already been used for the AWS Nitro Isolation Engine, the Atmosphere microkernel, CortenMM, Anvil, and others;
  it is 3–100× faster than the previous generation of verifiers, spec/proof is ghost code, **and there is zero runtime overhead after compilation**.
- **Ferrocene 26.02.0** extends certification from the compiler to a subset of the core library (ISO 26262 ASIL D / IEC 61508 SIL 3 / IEC 62304).
- Linux 7.0 (2026-04) announced that Rust support is permanent; the CISA 2026-01 deadline requires vendors to publish a memory safety roadmap;
  the EU Cyber Resilience Act pushes in the same direction.
- **Tock** has been adopted by Google Security Chip, OpenSK, and Microsoft Pluton; the UCSB team has already used Verus
  to perform **surgical verification** on Tock's virtual timer subsystem, proving that "retrofitting verification onto an existing system" is feasible.
- **Hubris** (Oxide): Rust + formal verification + message-passing isolation, in production use.

### Gap 3: Mixed-criticality has no usable engineering implementation

Running "µs-level hard real-time control" + "ms-level GUI / networking / OTA" on the same MCU at the same time is a real industrial requirement.
Industrial RTOSes (RT-Thread / Zephyr / RT-Linux) **only support the basics**, and the gap to deployment lies in functional safety certification and reference design maturity.
Academia has scheduling algorithms such as MC-CBS / MC-EDF, but **there is no mature open-source implementation targeting MCUs**.

### Gap 4: The engineering infrastructure for a Rust preemptive kernel is missing

The current state of the Rust embedded ecosystem is "cooperative-friendly, preemptive missing":

- Embassy: async/stackless, **cooperative within the same executor**, with cross-executor preemption relying on NVIC priority (a combination of hardware preemption and cooperative tasks).
  If one task spins for 5 ms without awaiting inside a single executor, it blocks the other tasks for 5 ms.
- RTIC: static priority, interrupt-driven, but not a complete OS.
- Tock: Rust + isolation, but limited scheduling capability and mostly cooperative.

**The pitfalls of a Rust preemptive kernel have already been publicly documented** (key engineering details): a context switch requires saving 34 registers (31 GPR + SP + ELR + SPSR),
about 300 cycles (Cortex-M, single address space); a real OS with NEON/FP, TLB, and page table switching costs 3000–5000 cycles.
RISC-V is more troublesome — **hardware automatic stacking saves only r0-r3/r12/lr/pc/xPSR**, and mepc has to be saved by hand-written `global_asm!`,
otherwise it degenerates into "interrupt handling" rather than "a real context switch". An existing open-source implementation (Rivet) has documented two real defects:
(1) interrupts already being enabled during the early boot stage caused the tick to corrupt task state;
(2) `mstatus.MPP` not being set to M-mode caused mret to trap into a U-mode loop fault —
**it presents as "task A was never scheduled", which looks nothing like a crash**.

---

## 4. Three Candidate Routes (for a Decision)

> What follows is **options and trade-offs**, not a recommendation. The final choice is made by the decision-maker.

### Route A: Preemptive Rust MCU Kernel + Partial Formal Verification

**What it does**
- Write a preemptive kernel in Rust, starting with Cortex-M33/M55 (with MPU / TrustZone / cache, close to the "high-performance MCU" target).
- POSIX-style API (pthread / socket / file descriptors), without pursuing the full POSIX surface.
- Use Verus to verify **the most critical invariants**: scheduler context switch, priority inheritance, mutex deadlock freedom.
  Follow the "surgical verification" route of the Tock paper instead of proving the whole kernel.
- The kernel **does not include** drivers and the file system (they are layered into user space), which is itself an isolation design.

**Trade-offs**
- ✅ Clearest differentiation: occupies the "Rust + verifiable + MCU + good enough" slot
- ✅ Clearly differentiated from Tock/Hubris: they are cooperative / minimal, we are preemptive / POSIX-style
- ⚠️ Enormous workload: the context switch, MPU domain management, and nested interrupts of a Rust preemptive kernel are all deep pitfalls
- ⚠️ Verus coverage is limited (Rust subset), not all code can enter the proof process
- ⚠️ No ecosystem: no drivers, no community, no application porting

**Reference coordinates**: Tock (Rust, isolation, non-preemptive), Hubris (Rust + formal, microkernel, in production),
the UCSB Tock+Verus paper (retrofit verification path)

---

### Route B: A "Predictability Middle Layer" on Top of Zephyr / NuttX

**What it does**
- Do not rebuild a kernel; build **the layer above**: exactly the kind of thing Zephyr's own comment talks about.
- Tasks declare runtime / period / deadline → the kernel or the middle layer does admission control (rejecting over-capacity requests).
- CBS (Constant Bandwidth Server)-style bandwidth reservation; over-quota is degraded instead of overrunning the deadline.
- Integrate measurement-based WCET (DWT->CYCCNT) + response time analysis, producing schedulability reports.
- Optional: MPU static partitioning, separating the "real-time plane" from the "general-purpose plane".

**Trade-offs**
- ✅ **Lowest risk**, borrowing Zephyr's 1000+ boards and full driver ecosystem; in the four-dimensional requirement set, "generality" comes for free
- ✅ Hits the quoted gap precisely (Zephyr explicitly says it guarantees no timing, and explicitly says "this can be implemented above it")
- ✅ Quantifiable output: admission control acceptance rate, a provable improvement driving the deadline miss count to zero
- ⚠️ Weak "building an OS" narrative, more like a toolchain / middleware
- ⚠️ Stops on top of someone else's kernel; the technical barrier is weaker than Route A
- ⚠️ Depends on upstream: changes to Zephyr's internal scheduler structures require a patch or a fork

**Reference coordinates**: Linux SCHED_DEADLINE / CBS, the partitioning idea of the Bao hypervisor, aiT's measurement-based alternative

---

### Route C: A Timing-Evidence Toolchain (AI + Forensics Route)

**What it does**
- A **timing evidence chain** for MCUs: every scheduling decision, context switch, and interrupt latency leaves an auditable record.
- Combine static/measurement-based analysis to give WCET bounds and response time analysis conclusions, producing a "predictability proof report".
- Can be combined with the existing agent toolchain (forensics / evidence-ladder methodology) to do automated timing regression.

**Trade-offs**
- ✅ Fits the existing capability accumulation (forensics methodology, agent automation) extremely well, the highest leverage
- ✅ No need to rebuild a kernel; Zephyr / NuttX / even bare metal all work
- ✅ Directly serves "predictability", the hardest dimension to quantify, and the deliverables are verifiable
- ⚠️ Leans toward tooling, not an OS, the weakest narrative tension
- ⚠️ Upper bound precision is limited by the measurement method (MBTA needs EVT statistics to argue representativeness), less sound than the static method

---

### Cross-Comparison of the Three Routes

| | A: Rust Verifiable Kernel | B: Predictability Middle Layer | C: Timing Evidence Toolchain |
|---|---|---|---|
| Differentiation strength | High | Medium | Medium-low |
| Deployment difficulty | Extremely high | Low | Medium |
| Source of generality | Build it ourselves (no ecosystem) | Borrow Zephyr (already there) | Irrelevant |
| Source of predictability | Formalization + proofs | Admission control + partitioning | Measurement + analysis |
| Source of reliability | Rust + partial proofs | Borrow upstream + admission | Evidence chain + regression |
| Verifiable output | Proof artifacts | Admission reports | Timing reports |
| Requires understanding of hardware scheduling | Yes (deep) | Medium | Medium |
| Fit with your existing stack | Medium (Rust familiar) | High (probe-rs / H7 / G431 all available) | Highest (forensics methodology already in place) |
| Project scale | School year | Semester | Semester |

---

## 5. Three Things That Must Be Decided First

Before writing the first line of code, the decision-maker must settle the following three points:

### 1. How to Resolve the Core Contradiction — This Is the Root of the Positioning

"General-purpose + real-time + reliable + predictable": the four cannot all be had at once, **one must be chosen as the first-class citizen**, the rest as constraints.

- **Generality first** (Zephyr route): the ecosystem wins, timing relies on documentation and engineering discipline, no formal guarantee.
- **Verifiability first** (seL4 route): correctness is proved, generality relies on building your own ecosystem, the entry barrier is the highest.
- **Predictability first** (new route): turn "no deadline miss" into a kernel-checkable invariant, generality is reduced by subtraction.

**Without this step, all the design work afterwards is spinning in place.**

### 2. Target Platform and Verifiability Budget

- ~~Starting platform: Cortex-M33/M55 (with MPU + TrustZone + cache)? Or M7 (without MPU)?~~
  → **Decided (2026-11-19, third round): STM32H723.**
  **Correction**: the previous version wrote "M7 without MPU" here, which is wrong. The Cortex-M7 MPU is an optional feature,
  and **ST implemented it in the H7 family** (verbatim from the official product page: *"STM32H723xE/G devices support ... a memory
  protection unit (MPU) to enhance application security"*). The H723 has an MPU, a 32KB D-Cache,
  550MHz, 564KB RAM, and Chrom-ART DMA2D (native support for indexed mode and a dedicated CLUT),
  so **no board change is needed and the static partitioning route is directly feasible**.
  Higher-end target (optional): STM32N6 (Cortex-M55, 800MHz, 4.2MB contiguous SRAM, TrustZone MPU,
  Neo-Chrom 2.5D GPU, LCD controller up to XGA).
- How large is the verifiability budget: writing Verus proofs is an order of magnitude slower than C,
  so it must be made explicit "which code enters the proof process and which is explicitly excluded".
  **Leaning toward: verify only the scheduler core invariants** (following UCSB's retrofit path, not pursuing whole-kernel proofs).

### 3. The Real Source of Generality

- ~~Borrow the Zephyr ecosystem (not building our own drivers)?~~
- ~~Build our own POSIX layer but accept "no drivers"?~~
- ~~Do not do POSIX; switch to "domain API + strong timing contracts" instead?~~

**Decided (2026-11-19, third round): scenario templates + tiered extensibility.**

- Generality comes from **L0-L2 tiered extensibility**: L0 built-in applications, L1 declarative scenario configuration, L2 Wasm plugins (bounded).
  **Loading native code at L3 is forbidden.**
- The extension mechanism is **assembled at compile time** rather than installed at run time, so the premise of "a fixed application set" is not broken,
  and the firmware remains a single hashable, signable artifact.
- API shape: **Plan 9-style namespaces + text ctl files + non-blocking message passing**
  (keeping Plan 9's uniformity, but changing its blocking semantics to fit hard real-time).

See `ARCHITECTURE.md`.

---

## 5.2 Positioning Revisions Added in the Third Round

The original framework of choosing one of routes A/B/C has been superseded by a more precise design:

> **A high-performance MCU real-time platform with a fixed application set, tiered extensibility, and whole-system verifiability.**

Differentiation does not land on any single mechanism (admission control, MPU partitioning, the Wasm sandbox, and the fixed application set each have their precedents),
but on **the superposition of the four, carried by the same Rust kernel**, which has no precedent on the MCU side:

| Reference | Verifiability | Path to generality | Missing |
|---|---|---|---|
| seL4 | Complete proof | Thin ecosystem, no path to generality | Generality |
| Zephyr | Explicitly abandons timing guarantees | Complete ecosystem | Predictability |
| NuttX | No formalization | Complete POSIX | Verifiability + heavy configuration burden |
| **ZayGO** | Partitioning + partial verification + admission reports | L0-L2 tiers | — |

### How the Four-Dimensional Goal Holds Simultaneously

| Goal | Implementation mechanism |
|---|---|
| Generality | L0-L2 tiered extensibility, L3 forbidden |
| Predictability | Fixed application set → WCET computable; fuel/memory upper bound → bounded plugins; admission control → over-capacity rejected |
| Reliability | MPU partitioning isolation; Non-Cacheable framebuffer; Rust memory safety; a single hashable artifact |
| Real-time performance | Bandwidth cap; non-blocking device API; write operations changed to posting a request |

### Key New Constraints (obtained in this round of forensics)

- The display goes through **L8 indexed + a 256-entry CLUT**, natively supported by the hardware (the H7 DMA2D has built-in CLUT memory).
- The framebuffer must be **Non-Cacheable** (MPU configuration), otherwise DMA2D reads torn data.
- The framebuffer **must not be placed in DTCM** (DMA1/DMA2 cannot reach it, only MDMA can).
- **Chinese fonts are the real cost item**: the full set is 2.1MB, but after compile-time subsetting it can be squeezed down to 1KB–142KB.

---

## 6. Suggested Next Actions (independent of the decisions above, can run in parallel)

1. **Minimum viability validation**: on an STM32H723 or Nucleo-G431, implement in Rust a
   **single-core scheduler prototype with deadline-based admission control**, and measure the actual behavior of "rejecting over-capacity requests".
   This single experiment validates at the same time: the cost of Rust preemptive switching, nested-interrupt timing, and the feasibility of the admission logic.
2. **Comparison baseline**: run Zephyr on the same MCU, measure its dispatch latency distribution with DWT->CYCCNT,
   and compare it against the prototype as the quantified baseline for "predictability improvement".
3. **Forensics first**: leave a trace of every measurement step of the experiments above, forming the first "timing evidence report" —
   this is both the prototype of Route C and the argument for Route B.

---

## Appendix: Sources

| Conclusion | Source |
|---|---|
| Zephyr provides no timing guarantee, it has to be implemented above | `zephyrproject-rtos/zephyr` `include/kernel.h` (k_thread_deadline_set doc comment) |
| Zephyr scheduling semantics (same-priority tiebreak) | Zephyr official documentation, Scheduling section |
| Zephyr 4.5 features and scale | zephyrproject.org release announcement (2026-10-28) |
| Zephyr provides no timing guarantee (third-party corroboration) | UCSB "The Time is Right: Retrofitting Formal Verification onto Timers in an Operating System" |
| seL4 AArch64 confidentiality proof completed | seL4 official news (2026-08-24), Proofcraft |
| seL4 16.0 / rust-sel4 5.0 / Microkit 2.3 | seL4 official news (2026-07-22) |
| NuttX version / scale / positioning | nuttx.apache.org download page; the Apache NuttX entry on hivebook.wiki |
| NuttX minimum footprint and the cost of POSIX | EET-China, "An Embedded Open-Source Project That Writes an RTOS Very Neatly — NuttX" (《一个把 RTOS 写得很规整的嵌入式开源项目——NuttX》) |
| Verus capabilities, speed, verified projects | Amazon Science, "Developing provably correct Rust code with Verus" (2026-08-31); AdaCore × CMU announcement |
| Current state of the Rust kernel ecosystem | the Embedded entry on rustprojectprimer.com; derekmolloy.ie, "The State of Rust for Embedded Development in Mid-2026" |
| Feasibility of Tock + Verus retrofit | the same UCSB paper |
| Details of Rust preemptive context switching and two real defects | dev.desigeek.com, "Building a microkernel in Rust (Part 3)"; the lib.rs/crates/rivet-macros documentation |
| Embassy cooperative semantics | Embassy Book; the comparison article on iotdigitaltwinplm.com |
| Deployment gap in mixed-criticality | gongkong.com, "Mixed-Criticality Scheduling for Industrial RTOS" (《工业 RTOS 的混合关键性调度》) |
| MCU timing analysis practice (measurement-based / static / hybrid) | embedcrest.com, dev.to/beefedai, AbsInt aiT |
| Rust preemptive kernel (academic prototype) | "Design and Implementation of a Rust Preemptive Kernel for Loongson Processors" (《面向龙芯处理器的 Rust 抢占式内核设计与实现》), Machine Industry Automation (《机械工业自动化》) 2026(09) |
