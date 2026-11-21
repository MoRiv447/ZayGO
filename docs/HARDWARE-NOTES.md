# 硬件笔记：STM32H723 USB Host 与显示相关

> 记录本项目涉及的外围硬件接线约束与踩坑点，避免重复查阅。
> 全部结论基于 2026-11-19 检索的官方手册与社区实践。
> 适用板子：**鹿小班 LXB723ZG-P1**（STM32H723ZGT6，LQFP144，1MB Flash / 564KB RAM），配 ST7789 SPI 屏。

---

## 〇、板子身份与实测确认

原理图：随开发板资料包提供的 `LXB723ZG-P1原理图.pdf`（本机路径不入库）
丝印映射：同目录 `排针和引脚丝印对应关系图.png`

从芯片读出的实测值（优先于文档）：

| 项 | 实测 |
|---|---|
| CPUID (`0xE000ED00`) | `0x411FC272` → Cortex-M7 **r1p2** |
| Flash 容量 (`0x1FF1E880`) | `0x0400` = **1024 KB** |
| DBGMCU_IDCODE (`0x5C001000`) | `0x10016483` → DEV_ID `0x483`（H72x/73x），REV `0x1001` |
| DBGMCU 地址 | **`0x5C001000`**，**不是** F4/F7 的 `0xE0042000`（后者读全 0） |
| 板内已有固件 | 是。VTOR=`0x08000000`、AIRCR.PRIGROUP=3（均非复位默认值） |

---

## 〇之二、板载外设分配（原理图确认）

| 功能 | 引脚 / 网络 | 说明 |
|---|---|---|
| **SPI 液晶**（FPC10-0.5 座 U7） | SCK=`PG13`、MOSI=`PG14`、CS=`PG8`、DC=`PG15`、BL=`PG12` | 板载 FPC 座；**但同组引脚也引到 2×30 排针，可用杜邦线直连 8 针模块** |
| 用户 LED（蓝） | **`PG7`**（1K 限流） | 可做不依赖任何外设的状态指示 |
| 电源 LED（红） | — | |
| **SWD + USART1 排针**（2×4P 卧贴 H1） | SWCLK=`PA14`、SWDIO=`PA13`、USART1_RX=`PA10`、USART1_TX=`PA9`、RST、3.3V | 调试口与串口在同一组排针 |
| **USB Type-C 16P**（U11） | 直连 `PA11`(USB0_N) / `PA12`(USB0_P)；**CC1/CC2 各 5.1kΩ 下拉**（R12/R13） | **只能是 Device（DFU/CDC），不能当 Host** |
| 扩展 Flash | W25Q64，OSPI，NCS=`PG6` | |
| TF 卡 | SDMMC1（`PC8`~`PC12`、`PD2`） | |
| DCMI 摄像头 | `PG9`/`PG10`/`PG11`、`PC6`/`PC7`、`PD3` | |
| 晶振 | **HSE 25MHz**（PH0/PH1）+ LSE 32.768kHz | 后续配 PLL 到 550MHz 有基础 |
| 电源 | +5V 输入 → **RY3420**（U13，buck：IN/EN/FB/SW）→ 3.3V | 排针上有 2 个 +5V 引出 |

### 对项目判断的三处修正

1. **ST7789 接线方式（本节曾判断错误，已更正）**
   板上确有 FPC10-0.5 液晶座（U7），但它**与常见的 8 针 ST7789 模块机械不兼容**，
   且随板未带 FPC 排线，该座实际不可用。
   好在 `IO口引出` 段的引脚清单包含 `PG12/PG13/PG14/PG15/PG8`：
   ```
   PG14 PG15 PG12 PG10 PG13 PG11 PD7 PD5 PG9
   PG8  PG5  PG3  PD14 PD15 PG2  PG4 PG7
   ```
   ⇒ **用杜邦线从排针直连 8 针模块，零成本，不需要 FPC 转接板。**
   FPC 座与排针并联在同一组网络，空置无影响。

   8 针模块接线：

   | 模块引脚 | 接板子 | 说明 |
   |---|---|---|
   | VCC | 3.3V | |
   | GND | GND | |
   | SCL / SCK | **PG13** | SPI6_SCK |
   | SDA / MOSI | **PG14** | SPI6_MOSI |
   | DC | **PG15** | LCD_DC |
   | CS | **PG8** | SPI6_NSS |
   | RES / RST | 3.3V 或任意 GPIO | 板上无独立 LCD_RST 网络 |
   | BLK / BL | 3.3V（常亮）或 **PG12** | 板上有 LCD_BL_PG12，可能经三极管驱动 |

   > 引脚顺序以模块自身丝印为准，各家排列不同，不要照抄表格顺序。

2. **PA9/PA10 确实是 USART1**（与 SWD 同组 2×4 排针），占用确认。但不影响 USB Host，
   因为 VBUS 检测本来就可省。
3. **Type-C 口不能当 USB Host** —— CC1/CC2 各挂 5.1kΩ 下拉，是标准 UFP（设备）配置，
   且没有 VBUS 供电路径。要做 Host 必须从 **IO 引出排针**取 `PA11`/`PA12` 并自行供 5V。

### 一个新机会：CDC 虚拟串口（暂缓）

Type-C 直连 PA11/PA12 ⇒ STM32 可自行实现 **USB CDC**，一根 Type-C 线同时供电与通信，
不需要 USB-TTL。但 Rust `no_std` 下要写 USB 设备栈（48MHz 时钟、端点配置、描述符），
工作量大，**不适合 P0/P1 阶段**。列为 P2 之后的产品形态优化项。

---

## 一、背景：Nucleo 与核心板的差异


NUCLEO-H723ZG 能直接当 USB Host，是因为板上有三件套；**核心板通常只引出 MCU 引脚，这三件都要自己补**。

| 器件 | 作用 | Nucleo (MB1364) | 核心板 |
|---|---|---|---|
| Micro-AB 座 | USB 物理接口 | CN13 | ❌ 需自备 |
| **VBUS 电源开关** | 5V 供电 + 限流 + 过流标志 | PD10 控制，PG7 报过流 | ❌ **必须补** |
| USBLC6-2 ESD 保护 | D+/D- 静电防护 | 板载 | ❌ 建议补 |
| USB 角色检测 ID | PA10 | CN13 pin4 | 固定 Host 可省 |

**Nucleo CN13 引脚定义**（UM2407 Table 16）：

| pin | 信号 | MCU pin | 说明 |
|---|---|---|---|
| 1 | VBUS | PA9 | 5V 供电，同时用于 VBUS 检测 |
| 2 | DM | PA11 | USB_OTG_HS_DM |
| 3 | DP | PA12 | USB_OTG_HS_DP |
| 4 | ID | PA10 | 角色检测（固定 Host 可省） |
| 5 | GND | — | |

Nucleo 板上 USB 相关焊桥（UM2407 Table 12）：
SB21（PA11）、SB22（PA12）、SB23（PA9）、SB24（PA10）、SB77（PD10 电源使能）、SB76（PG7 过流）。

---

## 二、H723 的 USB 外设

### 2.1 能力

- **USB 2.0 high-speed/full-speed device/host/OTG**，带专用 DMA
- **片上 FS PHY**（12 Mbps 够用，USB 鼠标是 FS 或 LS）
- ULPI 接口（接外部 HS PHY 用，本项目不需要）
- 相关引脚：PA11 = `OTG_HS_DM`、PA12 = `OTG_HS_DP`、PA9 = `OTG_HS_VBUS`、PA10 = `OTG_HS_ID`

**鼠标是 USB 全速/低速设备，用片上 FS PHY 即可，无需 ULPI 芯片、无需 48MHz 晶振。**

### 2.2 时钟要求（易错）

USB FS 要求参考时钟误差 **< 500ppm**。

H723 时钟树的 USB Clock Mux 可选：**PLL1Q / PLL3Q / HSI48**。

> **HSI48 精度不足**（内部 RC 源），必须走 PLL。
> 配错的表现：枚举失败、或「有时能识别有时不能」，极难排查。
> CubeMX 会做检查，但需确认生成结果里 PLL 分频正确。

### 2.3 VBUS 供电（ST 官方原文）

> "On-chip 5 V VBUS generation is not supported. For this reason, a charge pump or,
> **if 5 V are available on the application board, a basic power switch must be added
> externally to drive the 5 V VBUS line.** The external charge pump can be driven by any
> GPIO output. **This is required for host-only configuration.**"

主机侧还须满足：
- 自备 5V 电源给下游供电
- **限流保护**（防止下游短路拉垮内部电源轨）
- **过流检测**并上报控制器

### 2.4 VBUS 检测（AN4879）

PA9 是 5V 容忍引脚，但**绝对最大值不得超过 VDD + 4V**，且**禁止 MCU 未上电而 PA9 已接 5V**。

标准分压方案（VDD 在 3.0–3.6V）：

```
VBUS ──[ 33kΩ ]──┬──→ PA9
                 │
              [ 82kΩ ]
                 │
                GND
```

分压后约 1.7V。

> **用了分压就必须关闭外设的 VBUS 检测功能**（`OTG_GCCFG.VBDEN = 0`），
> 改用普通 GPIO 输入读取。AN4879 原文：
> "This method uses the regular GPIO functionality, the VBUS sensing function
> (OTG_GCCFG.VBDEN), embedded in PA9, should not be enabled."

**固定 Host 模式可省略整个检测**：上电即拉 EN 给 VBUS。ST 社区也有开发者确认：
*"Even without [Drive_VBUS_HS], I can still detect my USB."*

---

## 三、接线方案

### 3.0 关键补充：Host 模式必须外加 15kΩ 下拉（易漏）

**上一版遗漏项。** USB 规范中：

- **设备侧**：在 D+（全速）或 D-（低速）上拉 1.5kΩ，由设备自己加
- **主机侧**：**必须在 D+ 和 D- 各接 15kΩ 下拉到地**

主机端 15kΩ 下拉的作用：
1. 无设备接入时把总线偏置到 J 状态（idle）
2. 主机通过检测设备侧的 1.5kΩ 上拉来识别设备速度（FS 拉 D+，LS 拉 D-）
3. 防止总线浮空

SparkFun USB Host 教程原文：
> "Additionally, a **15kΩ pull-down resistor is needed on each D+ and D- line**.
> Attach one resistor from D+ to GND, and attach another from D- to GND."

components101 的 USB-A 母座说明同样要求 15kΩ 下拉。

> STM32 的 USB 外设**内置 1.5kΩ 上拉**（那是给 Device 模式用的），
> 但 **Host 模式所需的 15kΩ 下拉必须外加**。
> 漏掉的表现：能检测到供电但**枚举失败 / 设备不被识别**。

### 3.1 完整接线表

| USB-A 母座 | → | STM32H723 | 备注 |
|---|---|---|---|
| pin3 D+ | → | **PA12** | `OTG_HS_DP` |
| pin2 D− | → | **PA11** | `OTG_HS_DM` |
| pin1 VBUS | ← | 限流开关 OUT | 同时是母座 pin4 |
| pin4 VBUS | ← | 同 pin1 | USB-A 母座内部通常短接 |
| 外壳 | → | GND | 金属壳必须接地 |
| GND | ↔ | 核心板 GND | 共地必需 |

**D+ / D− 各加 15kΩ 下拉到 GND**（见 3.0）。

VBUS 供电与检测支路：

```
核心板 5V ──► TPS2051D IN
                 ├─ EN  ◄── 任意 GPIO（如 PB0）
                 ├─ OC  ──► 另一 GPIO（可选，过流标志）
                 └─ OUT ──► 母座 pin1 / pin4
                          └─► [33kΩ] ──► PA9 ──► [82kΩ] ──► GND
```

### 3.2 USB-A 母座引脚定义（已实测确认）

**实物确认（2026-11-19，淘宝「立式USB母座带PCB板 USB2.0母座已焊接好」，1.15 元）**：

板上丝印**从上到下**依次为：

```
┌─────────────┐
│  GND        │  → 核心板 GND
│  D−         │  → PA11   (+ 15kΩ 下拉到 GND)
│  D+         │  → PA12   (+ 15kΩ 下拉到 GND)
│  VBUS       │  ← TPS2051D OUT ← 核心板 5V
└─────────────┘
```

母座金属外壳同样接 GND。

> 该型号丝印清晰、方向明确，无需再实测。
> 若换用其他型号的转接板，仍需确认：USB-A 母座标准为
> pin1=VBUS / pin2=D− / pin3=D+ / pin4=GND或VBUS（厂家不同）。

**下单备注**：若买不到同款，建议选「USB-A 母座带 PCB」并在下单时问商家
"四个引脚从上到下的顺序是什么"。

### 3.3 限流开关选型

| 器件 | 封装 | 电流 | 说明 |
|---|---|---|---|
| **TPS2051B/D** | SOT-23-5 / SOIC-8 | 0.5A 连续，0.7–1.2A 限流 | 最常见，OC 为低有效开漏输出 |
| TPS22919 类负载开关 | SOT-23-6 | 限流内置 | 更小，无独立 OC |
| P-MOS + 限流电阻 | 分立 | 需自设计 | 成本最低但过流保护简陋 |

TPS2051 典型接法：`VIN` ← 5V，`OUT` → VBUS，`EN` ← GPIO（逻辑高导通），
`OC` → GPIO（低有效开漏，需上拉），输入 0.1µF / 输出 10µF 去耦。

**注意反向电流**：TPS2051B 支持 OUT→IN 反向流动但**不带限流**。
若母座侧可能由外部供电，需并联二极管或改用双向方案。

### 3.4 物料清单（USB Host 最小可用，已核实）

| 物料 | 数量 | 参考价 | 备注 |
|---|---|---|---|
| USB-A 母座带 PCB（立式，USB2.0） | 1 | 1.15 元 | **已确认型号**，丝印 GND/D−/D+/VBUS 自上而下 |
| TPS2051D（SOT-23-5） | 1 | ~1 元 | VBUS 限流 + 过流 |
| **15kΩ 电阻** | **2** | ~0.1 元 | **D+ / D- 下拉，必需** |
| 33kΩ / 82kΩ 电阻 | 各 1 | ~0.1 元 | VBUS 检测（固定 Host 可省） |
| 0.1µF + 10µF 电容 | 各 1 | ~0.2 元 | 开关去耦 |
| USB-A 公对公线 | 1 | 已有 | 接鼠标 |

**总计约 3 元。**

> **待确认**：核心板是否引出 **5V**？母座 VBUS 需要 5V 源。
> 若 5V 仅用于 LCD 背光且未引至排针，需从 5V 测试点飞线，
> 或加 3.3V→5V 升压模块（约 1 元）。

### 3.5 电容选型（本项目专用）

**结论：用 MLCC（独石/贴片陶瓷），材质 X7R 或 X5R，耐压 ≥10V。**

| 类型 | 特性 | VBUS 去耦表现 | 结论 |
|---|---|---|---|
| **独石/贴片 MLCC** | 陶瓷介质，ESR 极低，漏电流近零 | **最好**，能吸收 USB 插拔瞬态 | ✅ **用这个** |
| 瓷片电容（引脚式） | 同为陶瓷但体积大、ESL 高、容量难做大 | 勉强可用，寄生参数难看 | ⚠️ 备选 |
| 铝电解电容 | 容量大、**ESR 高**、有极性、漏电流大、寿命有限 | **不适合高频去耦**，仅适合储能 | ⚠️ 仅大容量场合 |

**关键差异**：铝电解 ESR 高 ⇒ 对高频瞬态（USB 插拔浪涌）响应慢。
USB 去耦必须靠低 ESR 的陶瓷。

#### 3.5.1 MLCC 直流偏压降容（最容易踩的坑）

X7R/X5R 材质的 MLCC 在直流偏压下**实际容量大幅下降**（介电常数随电场下降，固有物理特性，非质量问题）。

0805 封装典型实测：

| 标称 | 耐压 | 施加 5V 后实际容量 |
|---|---|---|
| 10µF | 6.3V | **仅剩 2–4µF**（掉 60%+） |
| 10µF | 16V | 约 6–8µF |
| 10µF | 25V | 约 8–9µF |

**规则：接 5V 的电容，耐压必须 ≥10V。**
**禁用 Y5V / Z5U 材质**（降容更狠，6.3V 的可能只剩 20%）。

#### 3.5.2 本项目电容采购表

| 用途 | 容量 | 耐压 | 材质 | 封装 | 数量 |
|---|---|---|---|---|---|
| 开关输入去耦 | 0.1µF (100nF) | ≥16V | X7R | 0603/0805 | 2 |
| 开关输出储能 | 10µF | **≥16V** | X7R 或 X5R | 0805/1206 | 2 |
| （可选）VBUS 稳压 | 100µF | 10V | **铝电解** | Φ6×11 | 1 |

- 0.1µF 用 0603 足够 —— USB 走线自带分布电容，**位置贴近芯片引脚比容量更重要**
- 10µF 用 0805 + **16V**（勿用 6.3V）
- 100µF 铝电解可选：让 VBUS 在短时掉电/浪涌时更"硬"，防鼠标瞬时掉电复位。
  USB 规范对 VBUS 电容有 **120µF 上限**（防浪涌电流过大烧主机），100µF 在范围内。
  **不加也能工作**，加了更稳。

#### 3.5.3 焊接注意

- TPS2051D 为 SOT-23-5，烙铁 ≤320℃，单颗几秒完成
- 电解电容**有极性**，长脚为正；接反会鼓包甚至爆
- 两个 15kΩ 下拉电阻直接焊在母座板背面（D+/D− 到 GND），省去飞线


**或**直接买「USB OTG Host 扩展板（含 TPS2051 + USB-A 母座）」，~8–15 元，只焊 4 根线。

### 3.5 软件栈的现实

| 路线 | 工作量 | 风险 |
|---|---|---|
| Rust 全自研 USB Host | 大 | 枚举、端点调度、描述符解析坑多 |
| ST 的 C 库 + Rust FFI | 中 | 引入 C 依赖，破坏"纯 Rust + 可验证"叙事 |
| **先用串口，后面再说** | **零** | 无 |

ST 的 USB Host 类驱动（UM1720 `USB_Host_Library/Class`）全部是 C 实现：
`usbh_hid.c` / `usbh_hid_parser.c` / `usbh_hid_mouse.c` / `usbh_hid_keybd.c` /
`usbh_msc.c` / `usbh_cdc.c`。


---

## 四、三个坑

### 坑 1：剪 Type-C 转接头时的二极管

Type-C 转 USB-A 的 OTG 适配器（手机接鼠标用的那种）内部走线：

```
USB-A pin3 (D+) ──► Type-C TX1 (A6/B6)
USB-A pin2 (D−) ──► Type-C RX1 (A7/B7)
USB-A pin1/4 (VBUS) ──► [可能有肖特基二极管] ──► Type-C VBUS (A4/A9/B4/B9)
```

**Type-C 侧还带 CC1/CC2 下拉电阻（Rd 5.1kΩ）**——手机用它判断"我是从设备，请给我供电"。

剪开 Type-C 那一头后可作 USB-A 引出线使用，但：

> **必须用万用表量 VBUS 路径的通断。**
> 若二极管在，电流方向被挡住，外部 5V 无法进入母座 VBUS —— 表现为"鼠标不亮"。
> 需短接该二极管。

**更干净的做法：直接买 4 脚 USB-A 母座**，避开二极管、CC 电阻、TX/RX 映射全部疑虑。

**注意**：Type-C 转 USB-A 适配器**没有 USB Host 供电能力**，
它只是数据转接，VBUS 需要外部提供。

### 坑 2：PA11/PA12 可能已被 CAN 占用

正点原子阿波罗 H7（STM32H743）的引脚分配（来自开源项目 BRINGUP.md）：

```
CAN1   PA11 RX / PA12 TX, 500 kbit/s
调试串口 USART1  PA9 TX / PA10 RX, 115200
```

**PA11/PA12 一旦被 CAN 占用，USB 就用不了。**
动线前必须查手上这块核心板的原理图确认。

同理 **PA9 常被用作 USART1_TX**（调试口），若要拿 PA9 做 VBUS 检测，
调试串口需换到其他引脚（如 PA2/PA3）。

### 坑 3：48MHz 时钟精度

见 2.2。**必须走 PLL，HSI48 不够准。**

---

## 五、ST 官方双角色范例（NUCLEO-H723ZG）

ST 社区文章《How to configure STM32 as USB dual role》要点：

- 验证平台正是 **NUCLEO-H723ZG**
- CubeMX **不原生支持双角色**，需建两个工程（Host 与 Device）再手工拷贝文件互用
- Host 工程配置：`USB_OTG_HS` → Host only（内部 FS PHY）→ Activate VBUS
- **Platform Settings** 里 `Drive_VBUS_HS` 需指定 GPIO（否则生成时弹警告，
  且不会生成 `usbh_platform.c`）
- 该文章用 USB 2.0 A-type female 转 micro-B male 线连接

配套的 USB Host 类驱动在 STM32Cube USB 中提供：

| 类 | 文件 |
|---|---|
| HID 鼠标与键盘 | `usbh_hid.c` / `usbh_hid_parser.c` / `usbh_hid_mouse.c` / `usbh_hid_keybd.c` |
| 大容量存储 | `usbh_msc.c` |
| CDC 虚拟串口 | `usbh_cdc.c` |

**参考坐标**：该范例在 NUCLEO 上完成，说明协议栈可行性已被官方验证；
核心板的增量工作是**供电电路 + 连接器**，协议栈工作量与 Nucleo 相同。

---

## 六、显示相关（ST7789 与 N647）

### 6.1 ST7789 无 CLUT

COLMOD (0x3A) 仅支持 **12/16/18 位**（RGB444 / RGB565 / RGB666），
**无 indexed 模式、无 CLUT 寄存器**。

⇒ L8 帧缓冲不能直驱，必须先展开为 RGB565：
- DMA2D 支持 indexed → RGB565 像素格式转换（硬件）
- SPI 带宽瓶颈：240×320×2 = 153KB/帧，SPI 40MHz ≈ 5MB/s ⇒ **全屏刷新约 30ms**
- ST7789 的 GRAM + CASET(0x2A)/RASET(0x2B) 窗口机制可只刷脏矩形以缓解

其他要点：
- 窗口地址必须用 CASET/RASET 设置，否则可能溢出显示区（表现为滚动条纹）
- 部分模组的物理原点不在 GRAM (0,0)（如 1.14 寸需从第 40 列起），
  优先用 CASET/RASET 加偏移，而非改 porch 寄存器
- COLMOD 配 0x05（16bit）+ RAMCTRL 低两位为 0 = RGB565 **大端序**（先高字节）
- 屏幕红蓝互换 → 查 MADCTL 的 RGB 位；绿块有水平细条纹 → 查字节序

### 6.2 LTDC 原生支持 L8 + 每层 CLUT

STM32 LTDC 手册（适用 H7 / N6）：

> "**Color look-up table (CLUT) up to 256 colors (256x24-bit) per layer**"
>
> 输入像素格式含 "**L8 (8-bit luminance or CLUT)**"、AL44、AL88

- CLUT 通过 `LTDC_LxCLUTWR` 逐层写入，**每层独立**
- LTDC 直接从 L8 帧缓冲读索引、硬件 CLUT 展开送屏，**CPU 零参与，无需 DMA2D 参与**
- AL44 格式只需加载 16 个颜色，索引按 4 位 L 通道重复为 8 位填充
  （L0→0x00，L1→0x11，L2→0x22，…… L15→0xFF）

### 6.3 两条显示路径的对照

| | H723 + ST7789 | N647 + 4.3" RGB LCD |
|---|---|---|
| 总线 | SPI（CPU/DMA 推像素） | 并行 RGB（LTDC 扫出） |
| L8 + CLUT | ✗ 需软件展开 | ✓ 硬件原生 |
| 刷屏 CPU 占用 | 高 | **零** |
| 触摸 | ✗ | ✓ GT911（I2C + 外部中断，5点，免校准） |
| 实时性影响 | **有**（总线竞争） | **几乎无** |
| 内存 | 150KB 双缓冲 / 564KB RAM | **768KB** 双缓冲 / 4.2MB + 32MB HyperRAM |

### 6.4 N6 额外图形硬件

- **Chrom-GRC (GFXMMU)**：可编程显示形状，只物理存储可见像素，4 个虚拟缓冲
- **Neo-Chrom GPU2D**：支持 L8、CLUT(256条目)、ARGB8888、RGB565/888 等；
  命令列表驱动 DMA，CPU 开销极低
- ST 官方：4.2MB RAM "could store a double frame buffer for a 1280 x 800 display"

### 6.5 显示缓存与内存位置（跨平台通用）

- **帧缓冲必须标 Non-Cacheable**（MPU 配置）。
  ST 笔记 DM00272913 列出四种解法；社区实测 TouchGFX 混用 AXI(write-through) +
  SDRAM(write-back) 即便把 cache 维护限制到帧缓冲范围**仍出现鬼影与 HardFault**。
  ⇒ 选 Non-Cacheable。
- **帧缓冲不能放 DTCM**：DMA1/DMA2 在 D2 域，访问不到 ITCM/DTCM（0x20000000），
  只有 MDMA（D1 域）可以。
- **LTDC 图层显存起始地址必须 16 字节对齐**，建议 32 字节对齐
  （cache line 为 32 字节，可避开 cache 维护的粒度陷阱）。

---

## 七、待办与决策记录

| 项 | 状态 |
|---|---|
| USB Host 硬件 | **推迟**。P0–P2 阶段用串口 CLI + 以太网 Web，不需要鼠标 |
| USB Host 输入抽象 | **现在就定死** `InputEvent` 统一事件流，加鼠标/触摸只是多一个 Source，UI 零改动 |
| H723 核心板引脚占用 | **待查**：PA11/PA12 是否被 CAN1 占用、PA9 是否被调试串口占用 |
| 以太网 | 若核心板带 PHY + RJ45（如阿波罗 H7 用 YT8512C/DP83848），插线即可，无需额外硬件 |

---

## 八、引用来源

| 结论 | 来源 |
|---|---|
| H723 USB 能力与引脚 | DS13313 数据手册 3.x、AN4879 |
| Nucleo CN13 引脚与 USB 焊桥 | UM2407《STM32H7 Nucleo-144 boards (MB1364)》Table 12 / Table 16 |
| 板上 VBUS 开关由 PD10 控制、PG7 报过流 | MB1364 原理图（deepbluembedded 拆解） |
| VBUS 必须外接电源开关 | ST 社区《Warning during STM3CubeMX setting in USB_HOST》 |
| 分压检测与 VBDEN 禁用 | AN4879《Introduction to USB hardware and PCB guidelines for STM32 MCUs》2.6 节 |
| VBUS 限流开关要求 | TI TPS2051B-Q1 数据手册 9.2.2 节 |
| 双角色官方范例 | ST 社区《How to configure STM32 as USB dual role》 |
| USB Host HID 类驱动清单 | UM1720《STM32Cube USB》 |
| ST7789 色彩格式与窗口机制 | ST7789V 手册、多个驱动实现案例 |
| **Host 模式需 15kΩ 下拉** | SparkFun《USB Type A Female Breakout Hookup Guide》、components101 USB-A 母座页 |
| USB-A 母座 pinout | components101《USB Type-A (Female) Connector》、ARIAT Tech breakout 说明 |
| LTDC 的 L8 与 CLUT | STM32 LTDC 手册、wiki.st.com LTDC 特性页 |
| D-Cache 一致性四种解法 | DM00272913《Level 1 cache on STM32F7/H7 series》 |
| DMA 访问不到 DTCM、鬼影案例 | ST 社区《DMA is not working on STM32H7 devices》《Granular D-Cache maintenance operations...》 |
| N647 板载 4.3 寸 RGB LCD + GT911 | 正点原子 N647 官方 wiki、ATK-MD0430R 模块仓库 |
| N6 的 Chrom-GRC / Neo-Chrom | STM32N647xx 数据手册 3.12 / 3.30 章节 |
| PA11/PA12 被 CAN 占用 | cgzjs/stm32h743-industrial-gateway BRINGUP.md |
