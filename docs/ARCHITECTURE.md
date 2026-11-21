# 架构补充：显示面、场景模板与分级开放

> 配套文档：`POSITIONING.md`（定位论证）。本文档记录第三轮讨论确定的设计约束。
> 状态：设计方向已定，细节待实现验证。全部结论基于 2026-11-19 检索到的公开信息。

---

## 零、对前序结论的两处更正

### 更正 1：STM32H7 有 MPU（上一轮说无，错误）

ST 官方 STM32H723 页面原文：

> "STM32H723xE/G devices support a full set of DSP instructions and **a memory protection unit (MPU)**
> to enhance application security."

Cortex-M7 的 MPU 是**可选**特性，ST 在 H7 系列（H723/H735/H743 等主流型号）实现了它。
MPU 能力：8 个主区域 + 每区域 8 个子区域、特权/非特权访问权限、Normal/Device/Strongly Ordered 内存类型属性。

**影响**：分区隔离方案**不需要更换到 M33/M55**，手上的 STM32H723 即可作为目标平台。

### 更正 2：H723 内存充足，帧缓冲不是瓶颈

STM32H723xE/G：550MHz Cortex-M7，1MB Flash，**564KB RAM**
（192KB 可在 ITCM/AXI 间共享 + 64KB 专 ITCM + 128KB 专 AXI + 128KB DTCM + 48KB AHB + 4KB 备份）。

参考 ST 对 STM32H7A3 的定位原话：*"1.4 MB of SRAM can store an entire frame buffer"*——图形是 ST 自己认定的 H7 主战场。

---

## 一、显示面设计：L8 + CLUT

### 1.1 硬件原生支持（已核实）

STM32H723 的 **Chrom-ART 加速器（DMA2D）** 手册原文：

> "All the classical color coding schemes are supported from 4-bit up to 32-bit per pixel
> **with indexed or direct color mode**... **The DMA2D has its own dedicated memories for CLUTs
> (color look-up tables)**."

**L8 + CLUT 完全对齐 DMA2D 原生工作模式**，不是软件模拟调色板。

### 1.2 帧缓冲体积对比

| 格式 | 320×240 | 480×272 | 800×480 | 备注 |
|---|---|---|---|---|
| RGB565（16bpp） | 150KB | 255KB | 750KB | 双缓冲翻倍 |
| RGB888（24bpp） | 225KB | 382KB | 1125KB | H723 装不下 |
| **L8 索引（8bpp）** | **75KB** | **127KB** | **375KB** | 配 256 项 CLUT（512B） |

L8 相对 RGB565 节省 50%，相对 RGB888 节省 66.7%。在 564KB RAM 上，L8 可做双缓冲（150KB 总占用），
RGB565 单缓冲都紧张。

### 1.3 L8 带来的三项独特能力

1. **换肤零成本**：改写 256×2 字节的 CLUT，整个界面配色即时切换。
   RGB565 做不到（颜色直接编码在像素里）。
2. **反色/夜间模式**：CLUT 反转即可，无需重绘。
3. **帧缓冲可哈希、可 diff**：每个字节是索引而非颜色，
   因此「这一帧和上一帧哪里变了」是字节级比较，不需要任何图像处理。

第 3 点与本项目的取证/证据链方法论天然契合。

### 1.4 显示面定位：查表而非渲染

L8 + CLUT 把「渲染」降维成「查表」：

```
传感器读数 ──→ 映射到索引 i（0..255）──→ CLUT[i] ──→ 屏幕颜色
```

- 复杂度从 O(布局算法) 降到 O(查表)
- 映射规则可预先验证（纯函数，可做属性测试）
- 索引空间是**有限的语义槽位**（温度档位、状态图标、进度条段位），
  天然可作为模板 schema 的一部分来声明

### 1.5 缓存一致性：L8 方案的隐藏代价（必须处理）

Cortex-M7 有 32KB D-Cache（write-back 默认）。DMA2D 与 LTDC **直读物理内存**，看不到 Cache 中的脏数据。

ST 应用笔记 DM00272913 原文：

> "If the CPU writes something to an area that has a write-back cache attribute, the write result is not
> seen on the SRAM as the access is buffered, and then if the DMA reads the same memory area to perform
> a data transfer, the values read do not match the intended data."

**已知踩坑实例**：ST 社区有 TouchGFX 用户将 cache 维护操作限制到帧缓冲范围后，
仍出现**鬼影（ghosting）与 HardFault**——因为帧缓冲跨越 AXI SRAM（write-through）与 SDRAM（write-back）
两个区域，混合策略下 `SCB_CleanInvalidateDCache_by_Addr` 不足以保证可见性。

**四种官方解法**：

| 方案 | 做法 | 代价 |
|---|---|---|
| A | MPU 将帧缓冲设为 **Non-Cacheable** | 简单；性能损失可接受；社区推荐 |
| B | 保留 Write-Back，写完显式 `SCB_CleanDCache_by_Addr` | 性能最高；**漏一处即花屏** |
| C | 改 Write-Through | 仅适合 TX；H74x/H75x r1p1 及更早有勘误 |
| D | 帧缓冲整体置于 Non-Cacheable 连续区 | 需满足对齐与连续性要求 |

**本项目决策：A**。理由：把帧缓冲划为独立 MPU 区域并标 Non-Cacheable，
一个显示 bug 不会污染其他内存，一个内存 bug 不会让 DMA2D 读到撕裂数据。
这与「可靠性」目标一致——用一点性能换确定的故障边界。

### 1.6 两条硬件硬约束（踩过才知道）

1. **DMA1/DMA2 访问不到 DTCM / ITCM**（0x20000000）。
   DMA1/DMA2 位于 D2 域；只有 MDMA（D1 域）能访问 TCM。
   ⇒ **帧缓冲必须放 AXI SRAM / SRAM1-3 / SDRAM，不能放 DTCM。**
2. **LTDC 图层显存起始地址必须 16 字节对齐**，建议 32 字节对齐
   （cache line 为 32 字节，32 字节对齐可避免 cache 维护操作的粒度陷阱）。

### 1.7 中文字体：真正的成本项

帧缓冲不是瓶颈，**字体才是**。实测数据：

| 方案 | 体积 | 渲染开销 |
|---|---|---|
| 全量 CJK TTF | 10MB+ | 需 FreeType 实时栅格化，不可行 |
| GB2312 全量 16×16 位图 | 216KB | memcpy + 位掩码，近零 |
| 全量 16px 中文库（LVGL） | 2.8MB | — |
| LVGL 16pt ASCII 95 字符 | 12KB | — |
| LVGL 中文基本区 20902 字符 | **2.1MB** | — |
| ESP32-C3 上 20px / 500 汉字子集 | **142KB** | 60FPS 保持 |
| 血氧仪实测：32 汉字 + 15 ASCII | **≈1KB** | 覆盖 100% UI 文本 |

**决策：字体在编译期按场景模板实际用到的字符串做子集化，生成位图字库烧进镜像。**
用 `pyftsubset` + `fonttools` 离线生成，输出为静态数组。

这与「应用集固定」是同一条逻辑的延伸：**场景模板声明了要显示什么文字，编译期就把字库裁好。**
若此链路不闭合，整个方案在中文场景下漏气。

### 1.8 显示面绝不可阻塞实时面

- 帧缓冲更新走 DMA2D，不占 CPU 做逐像素搬运
- 显示任务自身的优先级低于实时面，且带宽受限
- 显示故障（花屏、字库缺失）不得传播到实时面：MPU 分区隔离兜底

---

## 二、场景模板：通用性的真正来源

### 2.1 核心洞察

「不允许运行时安装软件」若走到极端，项目会退化为封闭垂直产品，通用性归零。
**编译期组装是解药**：

> 模板产出的仍然是一个固件镜像、一个可哈希可签名的制品、
> 一组有限可穷举的执行路径。**运行时不可变性完全保留，
> 但同一套内核能服务多个场景。**

这不是新发明，是被大规模验证过的模式：ESPHome 也是「编译进固件」而非「装到设备上」，
却服务了数百万个不同场景。

### 2.2 与 NuttX / Zephyr 配置体系的区别

| 设计选择 | NuttX | Zephyr | **本项目** |
|---|---|---|---|
| 配置载体 | Kconfig 菜单（组合爆炸） | Kconfig + Devicetree（**两套真相源**） | **单一声明式场景文件** |
| 真相源数量 | 一套 | 两套，需保持同步 | **一套** |
| 组合规模 | 1500+ 模板 | 数万配置项 | **数十个场景模板** |
| 产物 | 配置后的固件 | 配置后的固件 | 固件 + **可调度性报告** + SBOM |
| 主要短板（公开评价） | 配置负担重、学习曲线陡 | 两套真相源易不一致 | — |

NuttX 的 1500+ 模板被公开资料列为主要短板：
*"its hundreds of configuration options make maintenance substantially more complicated."*
本项目必须避开这个坑：**场景级模板数十个，而非配置项数千**。

### 2.3 场景模板的形态（示意）

```yaml
# scenes/guard-panel.zay
scene: guard-panel
display:
  width: 320
  height: 240
  format: l8-clut
  palette: industrial-dark
  fonts:                       # 编译期子集化输入
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

### 2.4 一份文件，三重身份

这是本项目最有价值的设计点：

1. **编译输入** —— 决定镜像里包含哪些模块
2. **准入控制的依据** —— 每个任务的 `budget` 是显式声明的，可参与可调度性分析
3. **可调度性报告的输入** —— 与分析工具输出对齐

「通用性」与「可预测性」被绑在**同一个文件**上，而不是两套互不相干的机制。

### 2.5 附带收益

- 场景 → 镜像 → SBOM → 哈希，一条链。
  SBOM 格式可借鉴 Zephyr 4.5 新增的 SPDX 3.0 支持。
- 同一内核、多个场景，各自有独立的可调度性报告与时序证据。
- 模板本身是声明式结构 ⇒ **schema 可形式化验证**，
  可与内核不变式的 Verus 验证并行推进。

---

## 三、分级开放：通用性不砍到零

「不允许安装软件」若走到极端，项目会从「平台」退化为「产品」，生态归零。
Plan 9 的教训正在此处——Drew DeVault 的评价：

> "Plan 9 failed, in a sense, because Unix was simply too big and too entrenched at the time."

因此保留分级扩展，但**原生代码装载必须禁止**：

| 层级 | 谁能写 | 能否装入设备 | 可预测性保证 |
|---|---|---|---|
| L0 内置应用 | 仅项目自身 | 编译进镜像 | 完整保证（可穷举） |
| L1 声明式场景配置 | 使用者 | 编译进镜像 | 高（不执行任意代码） |
| L2 Wasm 插件 | 第三方 | 运行时加载 | **有界**（fuel + 内存页双上界） |
| L3 原生代码 | 任何人 | **禁止** | 无 |

### 3.1 L2 为何可行：wasmi 的关键性质（已核实）

| 性质 | 说明 |
|---|---|
| 语言 | Rust（与内核同语言，无跨语言边界） |
| 部署 | `no_std` 可用，适配 MCU |
| **fuel metering** | 内置，给宿主界定模块可消耗多少执行量 |
| 内存 | 线性内存按页分配 ⇒ 内存占用有硬上界 |
| 执行模型 | 解释执行，**无 JIT**，无 W^X 问题 |
| 确定性 | 行为可复现，适合审计 |
| 合规 | 100% WebAssembly spec testsuite |
| 安全审计 | 两次外部审计：Runtime Verification Inc.（0.36–0.38，Stellar 基金会委托）、SRLabs（0.31，Parity Technologies 委托），报告 PDF 已提交仓库 |
| 版本 | 2.0.0 发布于 2026-09-01；2.0 将 fuel 计量绑定到输入 Wasm 而非内部字节码，使模块成本跨版本稳定 |
| 已知限制 | function references / GC / threads / exception handling 仍为 tracking issue，含这些特性的模块无法加载 |
| 生态 | 830 个 crate 直接/间接依赖；月下载量约 178 万 |

**为什么这个组合重要**：fuel + 内存页两个上界，正是 WCET 分析与 RAM 分析所需的输入。
⇒ 「通用」与「有界可预测」在 L2 层可以同时成立。

### 3.2 语言与工具链一致性

Wasm 是 Rust 的一等公民（`wasm32-unknown-unknown` 目标），内核、插件、宿主工具全在同一语言生态内。

---

## 四、场景模板 × 四平面：最终架构

```
┌─ 场景模板（编译期唯一入口）─┐      ┌─ 单一固件镜像 ─┐
│ 声明式场景文件              │─────▶│ 可哈希 可签名   │
│ 每任务显式声明 周期/截止/预算 │      │ 路径有限可穷举  │
│ 字体子集化输入              │      │ 可调度性报告    │
└────────────────────────────┘      │ SBOM 清单       │
                                    └─────────┬──────┘
┌─ Zay 运行时 · MPU 分区隔离 ────────────────┴──────┐
│ ┌─────────────────────────────────────────────────┐ │
│ │ 实时控制面   独占预算，零动态分配，不持锁等待     │ │
│ ├─────────────────────────────────────────────────┤ │
│ │ 显示控制器层 L8 统一帧缓冲 + 每层 256 项 CLUT     │ │  ← 第三轮修订
│ │              后端可选：LTDC 硬件扫出 / SPI 推送   │ │
│ │              帧缓冲区标 Non-Cacheable           │ │
│ ├─────────────────────────────────────────────────┤ │
│ │ 管理面       HTTP/WS 出数据，带宽上限，可被抢占  │ │
│ │              承载触摸事件处理与 UI 状态           │ │
│ ├─────────────────────────────────────────────────┤ │
│ │ 插件面       Wasm 沙箱，fuel + 内存页双上界      │ │
│ └─────────────────────────────────────────────────┘ │
│ ┌─────────────────────────────────────────────────┐ │
│ │ 统一设备 API  命名空间寻址 · 非阻塞消息传递      │ │
│ └─────────────────────────────────────────────────┘ │
└───────────────────────────────────────────────────┘
```

### 4.1 平面职责与约束汇总

| 平面 | 优先级 | 内存属性 | 带宽上限 | 能否触达硬件 |
|---|---|---|---|---|
| 实时控制面 | 最高 | TCM / 独占 | 独占预算 | **直接读写，唯一允许** |
| 显示控制器层 | 高但可让位 | Non-Cacheable 专用区 | LTDC 后端近乎为零；SPI 后端受限 | 只读快照，不改状态 |
| 管理面 | 低 | 普通可缓存 | 硬上限（如 20%） | 仅经设备 API 投递请求 |
| 插件面 | 最低 | 沙箱线性内存 | fuel 上界 | 仅经设备 API 投递请求 |

**关键规则：所有写操作都是「投递请求」而非「直接改状态」**，
状态变更统一由实时面执行并回执。这是把「Web/插件不可预测」这条威胁彻底封死的机制。

### 4.1b 显示控制器层：双后端设计（第三轮新增）

L8 帧缓冲作为**跨后端不变的统一中间格式**。上层渲染永远只写 L8 索引，
不感知后端差异；换 CLUT 即换肤，换后端不改一行渲染代码。

```
┌─ 显示控制器层（后端由场景模板选择）─────────────────┐
│ ┌────────────────────────┐ ┌──────────────────────┐ │
│ │ LTDC 后端（主力）       │ │ SPI 后端（成本档位）  │ │
│ │ 并行 RGB 屏，硬件扫出   │ │ 屏无 CLUT，需软件展开 │ │
│ │ CPU 零参与              │ │ DMA2D 做索引到 RGB565│ │
│ │ LTDC 原生 L8 + 每层     │ │ 总线竞争成为干扰源    │ │
│ │ 256 项 CLUT             │ │ 按脏矩形局部刷新      │ │
│ │ 800×480 L8 双缓冲 768KB │ │                      │ │
│ └────────────────────────┘ └──────────────────────┘ │
├──────────────────────────────────────────────────────┤
│ L8 统一帧缓冲 · Non-Cacheable · 32 字节对齐 · 不用 DTCM │
└──────────────────────────────────────────────────────┘
```

#### LTDC 原生支持 L8 + CLUT（关键取证结论）

STM32 LTDC 手册原文（适用于 H7 / N6）：

> "**Color look-up table (CLUT) up to 256 colors (256x24-bit) per layer**"
>
> 输入像素格式含 "**L8 (8-bit luminance or CLUT)**"、AL44、AL88

中文资料表述更直白：

> "L8（8-bit luminance or CLUT）8 位颜色格式，实际上仅仅是 8 位索引值，范围 0–255，
> 而每个索引值的具体颜色值在查色表 CLUT 里面存储。"

**这比 DMA2D 方案更直接**：LTDC 直接从 L8 帧缓冲读索引，用**硬件 CLUT**
展开成 RGB888/RGB565 送屏，CPU 零参与，**不需要 DMA2D 参与搬运**。
CLUT 通过 `LTDC_LxCLUTWR` 逐层写入，每层独立。

#### SPI 屏（ST7789）无 CLUT —— 硬限制

ST7789 的 COLMOD (0x3A) 仅支持 12/16/18 位（RGB444/RGB565/RGB666），
**无 indexed 模式、无 CLUT 寄存器**。

⇒ H723 + ST7789 上 L8 无法直驱，必须先展开为 RGB565：

- DMA2D 可做 indexed → RGB565 像素格式转换（硬件支持）
- **SPI 带宽成为瓶颈**：240×320×2 = 153KB/帧；SPI 40MHz 约 5MB/s
  ⇒ 全屏刷新约 30ms（约 30fps 上限），且需 CPU/DMA 全程参与
- ST7789 的 GRAM + CASET/RASET 窗口机制可只刷脏矩形以缓解

**结论：ST7789 可用，但属「SPI 屏 + 软件渲染」路线，不是 L8+CLUT 的验证平台。**

### 4.1c 两块板子的角色划分

| | H723 + ST7789 | N647 + 4.3" RGB LCD |
|---|---|---|
| 总线 | SPI（CPU/DMA 推像素） | **并行 RGB（LTDC 扫出）** |
| L8 + CLUT | ✗ 屏无 CLUT，需软件展开 | ✓ **LTDC 硬件 CLUT 原生支持** |
| 刷屏 CPU 占用 | 高（受 SPI 带宽限制） | **零** |
| 触摸 | ✗ 无 | ✓ GT911，I2C + 外部中断，5 点电容 |
| 分辨率 | 240×320 | 800×480（或 480×272） |
| 对实时性的影响 | **有**（总线竞争） | **几乎无**（纯 DMA） |
| 内存账 | L8 双缓冲 150KB / 564KB RAM | L8 双缓冲 **768KB** / 4.2MB SRAM + 32MB HyperRAM |
| 定位 | **成本敏感档位** | **主力旗舰档位** |

同一套内核、两种显示后端，由场景模板选择——这正是场景模板体系的用武之地。

#### N6 的额外图形硬件

- **Chrom-GRC (GFXMMU)**："Fully programmable display shape to physically store only the visible pixel"，
  4 个虚拟缓冲，各 3072/4096 字节每行 × 1024 行，可映射到任意系统内存；
  对圆形屏等非矩形显示可省内存。
- ST 官方原话：4.2MB RAM "could store a double frame buffer for a 1280 x 800 display"。
- **Neo-Chrom GPU2D**：支持格式含 **L8**、CLUT(256 条目)、ARGB8888、RGB565、RGB888、
  A8/A4、AI44、AI88；基于命令列表的 DMA，CPU 开销极低。

### 4.1d 触摸：新引入的实时性风险维度

GT911 为 **I2C + 外部中断 PEN**（I2C 地址由配置电阻或 OTP 决定，常见 0x14 / 0x5D）。

触摸是**外部异步输入**，若处理不当会直接冲击实时面。强制规则：

```
触摸中断（极高优先级，只做最少事）
  → 读 GT911 寄存器（I2C，短事务）
  → 投递事件消息给管理面
  → 管理面更新 UI 状态
  → 渲染写入 L8 帧缓冲
  → LTDC 硬件自动扫出
```

**硬规则**：

1. **中断服务程序内绝不允许绘制、绝不允许读传感器**。
   中断只做「读寄存器 + 投递消息」两件事。
2. **触摸 I2C 总线独占**。若同一 I2C 总线上还挂有实时传感器，
   管理面不得占用该总线——需在场景模板中显式声明总线归属。
3. **触摸事件只改变 UI 状态**，不直接改变设备状态。
   任何设备状态变更仍须经设备 API 投递到实时面执行。

> 触摸电容屏**无需校准**（正点原子原厂说明），省掉一道工序。


### 4.2 设备 API 映射（Plan 9 风格，改动阻塞语义）

Plan 9 的核心不是「everything is a file」，而是**每进程私有命名空间** +
**一切用文本文件控制** + **9P 打通本地与远程**。
其具体做法可直接借用：

- 建 TCP 连接：打开 `/net/tcp/clone` 取 id → 往 `/net/tcp/<id>/ctl` 写
  `connect 10.0.0.1!80` → 读写 `/net/tcp/<id>/data`
- **没有 socket 系统调用**，dial() 就是几个文件操作，可用 shell 脚本实现
- 设备控制走 `ctl` 文件写文本，而非二进制 ioctl ⇒ `echo` / `cat` 即可驱动硬件

| 现实对象 | Plan 9 风格接口 | 实时性要点 |
|---|---|---|
| 传感器 | `/dev/sensor/<id>/value`（读=采样） | 采样走 DMA/中断，不阻塞读 |
| 执行器 | `/dev/actuator/<id>/ctl`（写文本命令） | 写入即入队，由实时面消费 |
| 配置 | `/etc/<key>` | 静态只读分区 |
| 日志 | `/dev/log` | 环形缓冲，丢最旧而非阻塞 |
| 状态 | `/stat/<task>` | 只读快照 |
| 任务配额 | `/ctl/sched/admit` | 准入控制入口 |

**与 Plan 9 的关键差异**：Plan 9 的阻塞式文件语义不适合硬实时。
本项目改为**非阻塞消息传递**，但保留「命名空间 + 文本控制 + 一套协议」的统一性。

---

## 五、四维目标如何同时成立

上一轮指出「通用 + 实时 + 可靠 + 可预测」互斥。本轮的四项约束组合给出了解：

| 目标 | 靠哪一项约束实现 |
|---|---|
| 通用性 | L0-L2 分级开放（模板 + Wasm 插件），禁 L3 |
| 可预测性 | 应用集固定 → WCET 可算；fuel/内存上界 → 插件有界；准入控制 → 超容量即拒 |
| 可靠性 | MPU 分区隔离；Non-Cacheable 帧缓冲；Rust 内存安全；单一可哈希制品 |
| 实时性 | 带宽上限；无阻塞设备 API；写操作改为投递请求 |

**关键：不是靠单一机制，而是靠四者叠加。**
seL4 有可验证性与分区，但无通用性路径；
Zephyr 有通用性，但明确放弃时序保证；
本项目的组合在 MCU 侧尚无先例。

---

## 六、已定与待定

### 6.1 已定（第一轮）

- 设备**有屏幕**，但不渲染 HTML；显示走 **L8 索引 + 256 项 CLUT**
- L8 帧缓冲区 **Non-Cacheable**（MPU 配置），不用手动 cache 维护
- 帧缓冲置于 **AXI SRAM**，**不用 DTCM**（DMA2D/LTDC 访问不到）
- LTDC 图层地址 **32 字节对齐**
- 中文字体 **编译期子集化**为位图字库，按场景模板实际字符串裁剪
- 通用性来自 **L0-L2 分级**，**L3 原生代码装载禁止**
- 场景模板是**编译期唯一入口**，单一声明式文件

### 6.2 已定（第三轮，四项决策 + 硬件平台）

| 决策项 | 结论 | 理由摘要 |
|---|---|---|
| L2 Wasm 插件层 | **做** | wasmi 现成（Rust / no_std / fuel + 内存页双上界 / 两次外部审计）；保住通用性且不破坏「应用集固定」 |
| 设备 API | **Plan 9 式** | 命名空间 + ctl 文本文件 + 每进程私有命名空间；改阻塞语义为非阻塞消息传递以适配硬实时 |
| 准入判据 | **混合** | 实时面用静态 WCET（sound）；管理面用测量式 MBTA + EVT（轻） |
| 形式化验证范围 | **只验证调度器核心不变式** | 走 UCSB 的 retrofit 路径（给既有系统外科式加验证），不追全内核证明 |

**硬件平台（第三轮修订）**：

- **主力平台：STM32N647 + 4.3" RGB LCD（ATK-MD0430R）**
  - Cortex-M55 800MHz、4.2MB 内置 SRAM、32MB HyperRAM、32MB NOR Flash
  - LTDC 并行 RGB 直驱，**原生 L8 + 每层 256 项 CLUT**
  - GT911 电容触摸（I2C + 外部中断，5 点，**无需校准**）
  - 800×480 L8 双缓冲 768KB，内置 SRAM 放得下
  - **刷屏 CPU 零参与 ⇒ 对实时性影响近乎为零**
- **成本档位平台：STM32H723 + ST7789**
  - 550MHz、564KB RAM、有 MPU（**更正第一轮误判**）
  - SPI 屏无 CLUT ⇒ 需软件展开为 RGB565，**总线竞争成为干扰源**
  - 定位为低档位场景模板，非 L8+CLUT 验证平台
- 两块板覆盖**两种显示后端**，由场景模板选择——这正是模板体系的用武之地

### 6.3 仍待定（工程细节层，不阻塞启动）

1. **Verus 证明的具体不变式清单**：建议起步只证三条
   （① 上下文切换后寄存器状态完整；② 优先级继承无死锁；③ 准入控制不超容量）。
2. **可调度性分析工具链**：自研响应时间分析器，还是移植 Linux 的 CBS/`SCHED_DEADLINE` 语义。
3. **Wasm 宿主 ABI 的资源声明格式**：fuel 与内存页上界如何在场景模板中表达。
4. **非阻塞设备 API 的具体语义**：`/dev/xxx/read` 返回「数据未就绪」还是「部分数据 + 后续等待」，
   需与 Plan 9 的 `ctl` 文本风格统一。
5. **H723 是否保留为正式支持的第二平台**，还是仅作实验床。

---

## 七、引用来源

| 结论 | 来源 |
|---|---|
| H723 有 MPU、564KB RAM、550MHz | ST 官方 STM32H723VG 产品页与 DS13313 数据手册 |
| DMA2D 支持 indexed 模式与专用 CLUT 存储 | STM32H723/H735 数据手册 3.13 Chrom-ART Accelerator 章节 |
| **LTDC 原生支持 L8 + 每层 256×24bit CLUT** | STM32 LTDC 手册（wiki.st.com LTDC 特性页）；野火/正点原子 LTDC 中文资料 |
| D-Cache 与 DMA 一致性问题及四种解法 | ST 应用笔记 DM00272913《Level 1 cache on STM32F7/H7 series》 |
| DMA 访问不到 DTCM、缓存策略混用导致鬼影 | ST 社区《DMA is not working on STM32H7 devices》《Granular D-Cache maintenance operations cause artefacts or hardfaults》 |
| LTDC 对齐要求与缓存配置实践 | STM32 社区与 lpqz.cn《STM32H743 驱动 RGB 屏：LTDC 与 DMA2D 硬件加速实战》 |
| **ST7789 无 indexed 模式与 CLUT** | ST7789V 手册 COLMOD(0x3A) 定义；多个驱动实现案例（仅 0x05/0x06/12bit） |
| **N647 板载 4.3 寸 RGB LCD + GT911** | 正点原子 N647 官方 wiki（触摸屏实验章节）、ATK-MD0430R 模块仓库 README |
| N6 的 Chrom-GRC / Neo-Chrom / 4.2MB RAM | STM32N647xx 数据手册 3.12/3.13/3.30 章节；ST 官方博客 |
| 中文字体体积实测 | LVGL / NXP GUI Guider / U8g2 实战案例（16px 全量 2.8MB、20902 字符 2.1MB、500 汉字 142KB、32 汉字约 1KB） |
| wasmi 能力与审计 | lib.rs/crates/wasmi、hysenlabs 项目页、wasmi GitHub README（审计报告 PDF 已提交仓库） |
| Plan 9 私有命名空间与 9P 设计 | vectree.io Plan 9 词条、drewdevault.com《In praise of Plan 9》、9p.io 架构资料 |
| NuttX 配置负担与最小体积 | everythingxiaomi.com NuttX 评测、hivebook.wiki Apache NuttX 词条 |
| ESPHome 编译期模型与规模 | esphome.io 官方文档 |
| Zephyr 4.5 SPDX 3.0 SBOM | zephyrproject.org 4.5 发布公告（2026-10-28） |

