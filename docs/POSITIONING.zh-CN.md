# 项目定位分析：通用实时内核（暂定名 Zay）

> 中文版。 [English version](POSITIONING.md)

> 状态：立项前的定位论证。全部结论基于 2026-11-19 检索到的公开信息。
> 方法：先取证三个参照项目的实际能力边界，再定位无人占据的位置，最后列出可选路线与代价。
> **本文档不替决策者做选择，只把选项与取舍摆清楚。**

---

## 一、问题陈述

目标：在高性能 MCU（Cortex-M7/M33/M55、RISC-V 等）上，提供同时具备**通用性、实时性、可靠性、可预测性**的操作系统内核，
形态上类似「GPOS 与 RTOS 的结合体」。

需要先回答两个问题：

1. 这个方向的**真实空白**在哪里？现有项目（Zephyr / seL4 / NuttX）做不到的是什么？
2. 项目应当**如何定位**，才能既有差异化，又不至于沦为一个「又一个 RTOS」？

---

## 二、参照项目取证（截至 2026-11-19）

### 2.1 能力边界对照表

| 维度 | Zephyr 4.5 | seL4 16.0 | NuttX 13.1 |
|---|---|---|---|
| 定位取向 | 广度优先 | 正确性优先 | POSIX 完整度优先 |
| 生态规模 | 1000+ 板子、11 种架构（2026 新增 TriCore / Hexagon）、单版本 1200+ 贡献者 | 平台少，但有 Microkit / CAmkES / capDL / rust-sel4 全套工具链 | 300+ 板子、1500+ 配置模板、15+ CPU 架构 |
| 进程模型 | 线程 + 可选 userspace（MPU） | 微内核 + capability 系统 + MCS 调度 | POSIX 进程（可 flat 或 protected）、fork / mmap / socket |
| 网络 / 存储 | 全栈（lwIP），4.5 新增 QUIC / HTTP3 / SSH / IPv4 转发 NAT / WireGuard | 依赖用户态组件（NetBSD 等） | BSD socket + 可选 littlefs / ROMFS / VFAT / NFS |
| 形式化验证 | 无 | 内核功能正确性 + 完整性 + 保密性（AArch64 全证明，2026-08 完成保密性） | 无 |
| 内存安全 | 无（Zephyr 用 C） | 有（内核为 C，但有证明 + Rust 支持 5.0） | 无（Apache-2.0，C） |
| 最小体积 | 视配置，远大于 RTOS | 内核极小（微内核） | 约 100 KB Flash / 16 KB RAM 起（最小配置约 32 KB） |
| 落地证据 | 1000 万+ 设备；Bosch EDC17 发动机 ECU（TriCore） | 高保证嵌入式（基站、飞控、安全关键） | PX4 无人机、SONY TizenRT、小米 Vela |
| 社区治理 | Linux Foundation，厂商会员主导 | seL4 Foundation（UNSW 主导，ANU 于 2026-09 加入） | Apache 顶级项目，2–10 人核心 |

### 2.2 关键证据摘录

**Zephyr 官方明确拒绝做时序保证。** `include/kernel.h` 中 `k_thread_deadline_set()` 的文档注释原文：

> "Despite the API naming, the scheduler makes no guarantees the thread WILL be scheduled within that deadline,
> nor does it take extra metadata (like e.g. the 'runtime' and 'period' parameters in Linux `sched_setattr()`)
> that allows the kernel to validate the scheduling for achievability. **Such features could be implemented
> above this call, which is simply input to the priority selection logic.**"

解读：Zephyr 的 `CONFIG_SCHED_DEADLINE` 只是**同优先级内的择序 tiebreak**，不是真正的 EDF 调度器；
它**不收 runtime/period 元数据**，因此**内核自身无法做准入控制与可调度性判定**。
这是 Zephyr 在「通用 ↔ 可预测」权衡上明确选了通用。

**seL4 的证明不覆盖应用与驱动。** seL4 证明的是内核自身的功能正确性、完整性、保密性。
WCET 方面，seL4 与 CertiKOS 是学术界的先例，但**内核 WCET 不含驱动与应用**；
应用层时序仍需自行分析（通常依赖 aiT / RapiTime 等商业工具）。

**NuttX 明确不追求 Linux 级广度。** 官方立场：小体积优先，Linux 级广度不是目标。
代价体现在：最小配置约 32 KB RAM，1500+ 配置模板带来显著的配置学习成本。

**Zephyr 在时序保证上的表述被学术界直接质疑。** 加州大学戴维斯分校 2026 年论文
《The Time is Right: Retrofitting Formal Verification onto Timers in an Operating System》原文：

> "The Zephyr RTOS, a popular open source RTOS, **provides no guarantees at all on timing** other than
> experimental benchmarks. This level of assurance is not enough for critical systems where failure can cost
> millions of dollars. The seL4 and CertiKOS operating systems formally verify their worst case execution time (WCET)."

---

## 三、真正的空白：三者共同缺失的东西

三家不是同质竞品，而是**三种互斥的取舍策略**（通用 / 可验证 / POSIX 完整）。
因此「再造一个通用+实时二合一」在**内核设计层面已被占满**，纯内核路线缺乏差异化。

但三家存在一个共同的结构性缺口：

### 缺口 1：通用性与可预测性之间缺少「调和机制」

| 机制 | 做什么 | 代表实现 | 谁有这个 | 谁没有 |
|---|---|---|---|---|
| **准入控制** | 任务申请时做可调度性分析，超容量则拒绝，而非事后超期 | CBS / SCHED_DEADLINE | Linux（AP 侧成熟） | 三家 MCU RTOS 全无 |
| **静态分区** | 用 MPU / 域把通用代码与硬实时代码物理隔离，各自独立分析 | Bao hypervisor、seL4 domains | seL4（较重） | Zephyr / NuttX 无分区抽象 |
| **静态时序分析** | 求 WCET 上界 + 响应时间分析，证明不超期 | aiT / Bound-T / RapiTime | 商业工具（2 万–10 万美元） | 三家均无集成 |

> 关键点：Zephyr 的注释明说「Such features could be implemented above this call」——
> **它把门开着，只是没人做。**

### 缺口 2：MCU 上「可验证」与「够用」的中间地带无人占据

- seL4 路线可验证，但生态薄、门槛高、学习曲线陡（capability / capDL / Microkit）。
- NuttX 路线够用，但无形式化保证、无功能安全认证。
- Zephyr 路线通用，但**明确放弃时序保证**（见 2.2 引用）。

**中间地带存在**：一个「Rust 内核 + 局部形式化验证 + POSIX 风格 API + 可在 MCU 上实际跑起来」的系统。

Rust 侧的条件在 2026 年已经成熟：

- **Verus**（CMU + Amazon）已开源，可验证 Rust 的函数正确性而非仅内存安全；
  已用于 AWS Nitro Isolation Engine、Atmosphere 微内核、CortenMM、Anvil 等；
  比前代验证器快 3–100 倍，spec/proof 为 ghost code，**编译后零运行时开销**。
- **Ferrocene 26.02.0** 把认证从编译器扩展到 core library 子集（ISO 26262 ASIL D / IEC 61508 SIL 3 / IEC 62304）。
- Linux 7.0（2026-04）宣布 Rust 支持永久化；CISA 2026-01 截止日期要求厂商公布内存安全路线图；
  EU Cyber Resilience Act 同向施压。
- **Tock** 已被 Google Security Chip、OpenSK、Microsoft Pluton 采用；UCSB 团队已用 Verus
  对 Tock 的虚拟定时器子系统做**外科式验证**（surgical verification），证明「给既有系统 retrofit 验证」可行。
- **Hubris**（Oxide）Rust + 形式化验证 + 消息传递隔离，量产使用。

### 缺口 3：混合临界性（mixed-criticality）缺可用的工程化实现

同一个 MCU 上同时跑「µs 级硬实时控制」+「ms 级 GUI / 网络 / OTA」是真实工业需求。
工业 RTOS（RT-Thread / Zephyr / RT-Linux）**只支持基础件**，落地差距在功能安全认证与参考设计成熟度。
学术界有 MC-CBS / MC-EDF 等调度算法，但**没有面向 MCU 的成熟开源实现**。

### 缺口 4：Rust 抢占式内核的工程基础设施缺位

Rust 嵌入式生态的现状是「协程友好、抢占缺位」：

- Embassy：async/stackless，**同一 executor 内为协作式**，跨 executor 靠 NVIC 优先级抢占（硬件抢占 + 协作任务的组合）。
  单 executor 内若一个任务自旋 5 ms 不 await，会阻塞其他任务 5 ms。
- RTIC：静态优先级、中断驱动，但非完整 OS。
- Tock：Rust + 隔离，但调度能力有限，且协作式为主。

**Rust 抢占式内核的坑已被公开记录**（关键工程细节）：上下文切换需保存 34 个寄存器（31 GPR + SP + ELR + SPSR），
约 300 cycles（Cortex-M，单地址空间）；真实 OS 含 NEON/FP、TLB、页表切换则 3000–5000 cycles。
RISC-V 更棘手——**硬件自动压栈只保存 r0-r3/r12/lr/pc/xPSR**，mepc 需手写 `global_asm!` 保存，
否则会退化成「中断处理」而非「真正的上下文切换」。已有开源实现（Rivet）记录了两个真实缺陷：
（1）早期启动阶段中断已使能导致 tick 破坏任务状态；
（2）`mstatus.MPP` 未设置为 M-mode 导致 mret 陷入 U-mode 循环 fault——
**表现为「任务 A 从未被调度」，看起来完全不像崩溃**。

---

## 四、三条候选路线（供拍板）

> 以下是**选项与取舍**，不是推荐。最终由决策者选择。

### 路线 A：抢占式 Rust MCU 内核 + 局部形式化验证

**做什么**
- Rust 编写抢占式内核，Cortex-M33/M55 起步（带 MPU / TrustZone / cache，贴近「高性能 MCU」）。
- POSIX 风格 API（pthread / socket / 文件描述符），不追求全量 POSIX。
- 用 Verus 对**最关键的不变式**做验证：调度器上下文切换、优先级继承、互斥量无死锁。
  走 Tock 论文的「外科式验证」路线，而非全内核证明。
- 内核**不含**驱动与文件系统（分层到用户态），这本身就是隔离设计。

**取舍**
- ✅ 差异化最清晰：占据「Rust + 可验证 + MCU + 够用」空位
- ✅ 与 Tock/Hubris 差异化明确：它们是协作式 / 极简，我们是抢占式 / POSIX 风格
- ⚠️ 工作量极大：Rust 抢占式内核的上下文切换、MPU 域管理、中断嵌套都是深坑
- ⚠️ Verus 覆盖率有限（Rust 子集），不是所有代码都能进证明流程
- ⚠️ 无生态：无驱动、无社区、无应用移植

**参考坐标**：Tock（Rust，隔离，非抢占）、Hubris（Rust + 形式化，微内核，量产）、
UCSB Tock+Verus 论文（retrofit 验证路径）

---

### 路线 B：在 Zephyr / NuttX 之上做「可预测性中间层」

**做什么**
- 不重造内核，做**上面那层**：Zephyr 自己注释里说的那种东西。
- 任务声明 runtime / period / deadline → 内核或中间层做准入控制（拒绝超容量申请）。
- CBS（Constant Bandwidth Server）风格的带宽预留，超额即降级而非超期。
- 集成测量式 WCET（DWT->CYCCNT）+ 响应时间分析，产出可调度性报告。
- 可选：MPU 静态分区，把「控制面」与「通用面」隔开。

**取舍**
- ✅ **风险最低**，借 Zephyr 1000+ 板子与全驱动生态，四维需求里「通用性」直接白拿
- ✅ 精确命中被引用的空白（Zephyr 明说不保证 timing，且明说「可以在这之上实现」）
- ✅ 产出可量化：准入控制接受率、超期次数归零的可证明改善
- ⚠️ 「造 OS」的叙事弱，更像工具链 / 中间件
- ⚠️ 停在别人内核之上，技术壁垒不如路线 A
- ⚠️ 依赖上游：Zephyr 调度器内部结构改动需 patch 或 fork

**参考坐标**：Linux SCHED_DEADLINE / CBS、Bao hypervisor 的分区思路、aiT 的测量式替代方案

---

### 路线 C：时序证据化工具链（AI + 取证路线）

**做什么**
- 面向 MCU 的**时序证据链**：每次调度决策、上下文切换、中断延迟都留下可审计记录。
- 结合静态/测量分析给出 WCET 界与响应时间分析结论，产出「可预测性证明报告」。
- 可与既有 agent 工具链（取证 / 证据阶梯方法论）结合，做自动化时序回归。

**取舍**
- ✅ 与现有能力积累（取证方法论、agent 自动化）高度契合，杠杆最高
- ✅ 不需要重造内核，用 Zephyr / NuttX / 甚至裸机皆可
- ✅ 直接服务于「可预测性」这一最难量化的一维，产出物可验证
- ⚠️ 偏工具，不是 OS，叙事张力最弱
- ⚠️ 上界精度受限于测量法（MBTA 需 EVT 统计论证代表性），不如静态法 sound

---

### 三条路线横向对比

| | A：Rust 可验证内核 | B：可预测性中间层 | C：时序证据工具链 |
|---|---|---|---|
| 差异化强度 | 高 | 中 | 中低 |
| 落地难度 | 极高 | 低 | 中 |
| 通用性来源 | 自己造（无生态） | 借 Zephyr（直接有） | 无关 |
| 可预测性来源 | 形式化 + 证明 | 准入控制 + 分区 | 测量 + 分析 |
| 可靠性来源 | Rust + 局部证明 | 借上游 + 准入 | 证据链 + 回归 |
| 可验证产出 | 证明工件 | 准入报告 | 时序报告 |
| 是否需要懂硬件调度 | 是（深） | 中 | 中 |
| 与你现有栈的契合 | 中（Rust 熟） | 高（probe-rs / H7 / G431 都有） | 最高（取证方法论现成） |
| 项目规模 | 年级 | 学期 | 学期 |

---

## 五、必须先定的三件事

在写第一行代码前，以下三点必须由决策者拍板：

### 1. 核心矛盾怎么解——这是定位的根

「通用 + 实时 + 可靠 + 可预测」四者不可兼得，必须选**一个作为一等公民**，其余作为约束。

- **通用优先**（Zephyr 路线）：生态取胜，时序靠文档与工程纪律，无形式化保证。
- **可验证优先**（seL4 路线）：正确性有证明，通用性靠自建生态，门槛最高。
- **可预测优先**（新路线）：把「不超期」变成内核可检查的不变式，通用性做减法。

**没有这一步，后面所有设计都是空转。**

### 2. 目标平台与可验证性预算

- ~~起步平台：Cortex-M33/M55（带 MPU + TrustZone + cache）？还是 M7（无 MPU）？~~
  → **已定（2026-11-19 第三轮）：STM32H723。**
  **更正**：上一版此处写「M7 无 MPU」是错的。Cortex-M7 的 MPU 是可选特性，
  **ST 在 H7 系列实现了它**（官方产品页原文：*"STM32H723xE/G devices support ... a memory
  protection unit (MPU) to enhance application security"*）。H723 有 MPU、有 32KB D-Cache、
  550MHz、564KB RAM、Chrom-ART DMA2D（原生支持 indexed 模式与专用 CLUT），
  **无需换板，静态分区路线直接可行**。
  更高阶目标（可选）：STM32N6（Cortex-M55，800MHz，4.2MB 连续 SRAM，TrustZone MPU，
  Neo-Chrom 2.5D GPU，LCD 控制器至 XGA）。
- 可验证性预算是多少：Verus 证明写起来比 C 慢一个数量级，
  需明确「哪些代码进证明流程、哪些明确排除」。
  **倾向：只验证调度器核心不变式**（走 UCSB 的 retrofit 路径，不追求全内核证明）。

### 3. 通用性的真实来源

- ~~借 Zephyr 生态（不自造驱动）？~~
- ~~自造 POSIX 层但接受「无驱动」？~~
- ~~不做 POSIX，改为「领域 API + 强时序契约」？~~

**已定（2026-11-19 第三轮）：场景模板 + 分级开放。**

- 通用性来自 **L0-L2 分级开放**：L0 内置应用、L1 声明式场景配置、L2 Wasm 插件（有界）。
  **L3 原生代码装载禁止**。
- 扩展机制是**编译期组装**而非运行时安装，因此「应用集固定」的前提不被破坏，
  固件仍是单一可哈希可签名制品。
- API 形态：**Plan 9 式命名空间 + 文本 ctl 文件 + 非阻塞消息传递**
  （保留 Plan 9 的统一性，但改掉其阻塞语义以适配硬实时）。

详见 `ARCHITECTURE.md`。

---

## 五之二、第三轮补充的定位修订

原路线 A/B/C 三选一的框架已被更精确的设计取代：

> **一个应用集固定、分级可扩展、整体可验证的高性能 MCU 实时平台。**

差异化不落在单点机制上（准入控制、MPU 分区、Wasm 沙箱、固定应用集各有先例），
而落在**四者叠加、且由同一套 Rust 内核承载**，在 MCU 侧尚无先例：

| 参照 | 可验证性 | 通用性路径 | 缺失 |
|---|---|---|---|
| seL4 | 完整证明 | 生态薄，无通用路径 | 通用性 |
| Zephyr | 明确放弃时序保证 | 完整生态 | 可预测性 |
| NuttX | 无形式化 | POSIX 完整 | 可验证性 + 配置负担重 |
| **Zay** | 分区 + 局部验证 + 准入报告 | L0-L2 分级 | — |

### 四维目标如何同时成立

| 目标 | 实现机制 |
|---|---|
| 通用性 | L0-L2 分级开放，禁 L3 |
| 可预测性 | 应用集固定 → WCET 可算；fuel/内存上界 → 插件有界；准入控制 → 超容量即拒 |
| 可靠性 | MPU 分区隔离；Non-Cacheable 帧缓冲；Rust 内存安全；单一可哈希制品 |
| 实时性 | 带宽上限；无阻塞设备 API；写操作改为投递请求 |

### 关键新约束（本轮取证所得）

- 显示走 **L8 索引 + 256 项 CLUT**，硬件原生支持（H7 DMA2D 内置 CLUT 存储器）。
- 帧缓冲必须 **Non-Cacheable**（MPU 配置），否则 DMA2D 读到撕裂数据。
- 帧缓冲**不能放 DTCM**（DMA1/DMA2 访问不到，只有 MDMA 能）。
- **中文字体是真正成本项**：全量 2.1MB，编译期子集化后可压到 1KB–142KB。

---

## 六、下一步建议动作（不依赖上述拍板，可并行）

1. **最小可行性验证**：在 STM32H723 或 Nucleo-G431 上，用 Rust 实现一个
   **带 deadline 准入控制的单核调度器原型**，测量「拒绝超容量申请」的实际行为。
   这一个实验能同时验证：Rust 抢占式切换成本、中断嵌套时序、准入逻辑可行性。
2. **对比基线**：同一 MCU 上跑 Zephyr，用 DWT->CYCCNT 测其调度延迟分布，
   与原型对比，作为「可预测性改善」的量化基线。
3. **取证先行**：把上述实验的每一步测量留痕，形成第一份「时序证据报告」——
   这既是路线 C 的原型，也是路线 B 的论据。

---

## 附：引用来源

| 结论 | 来源 |
|---|---|
| Zephyr 无时序保证、需上层实现 | `zephyrproject-rtos/zephyr` `include/kernel.h`（k_thread_deadline_set 文档注释） |
| Zephyr 调度语义（同优先级 tiebreak） | Zephyr 官方文档 Scheduling 章节 |
| Zephyr 4.5 特性与规模 | zephyrproject.org 发布公告（2026-10-28） |
| Zephyr 无时序保证（第三方印证） | UCSB《The Time is Right: Retrofitting Formal Verification onto Timers in an Operating System》 |
| seL4 AArch64 保密性证明完成 | seL4 官网新闻（2026-08-24），Proofcraft |
| seL4 16.0 / rust-sel4 5.0 / Microkit 2.3 | seL4 官网新闻（2026-07-22） |
| NuttX 版本 / 规模 / 定位 | nuttx.apache.org 下载页；hivebook.wiki Apache NuttX 词条 |
| NuttX 最小体积与 POSIX 代价 | EET-China《一个把 RTOS 写得很规整的嵌入式开源项目——NuttX》 |
| Verus 能力、速度、已验证项目 | Amazon Science《Developing provably correct Rust code with Verus》（2026-08-31）；AdaCore × CMU 公告 |
| Rust 内核生态现状 | rustprojectprimer.com Embedded 词条；derekmolloy.ie《The State of Rust for Embedded Development in Mid-2026》 |
| Tock + Verus retrofit 可行性 | UCSB 同上论文 |
| Rust 抢占式上下文切换细节与两个真实缺陷 | dev.desigeek.com《Building a microkernel in Rust (Part 3)》；lib.rs/crates/rivet-macros 文档 |
| Embassy 协作式语义 | Embassy Book；iotdigitaltwinplm.com 对比文 |
| 混合临界性落地差距 | gongkong.com《工业 RTOS 的混合关键性调度》 |
| MCU 时序分析实践（测量法 / 静态法 / 混合） | embedcrest.com、dev.to/beefedai、AbsInt aiT |
| Rust 抢占式内核（学术原型） | 《面向龙芯处理器的 Rust 抢占式内核设计与实现》，机械工业自动化 2026(09) |
