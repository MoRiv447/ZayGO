# 实验 01：抢占式调度器的干扰量化

> 中文版。 [English version](README.md)

在写完整内核之前，用一个最小可运行的抢占式调度器测出：
**低优先级负载会把高优先级任务的调度延迟拖到多少？**

这个数字会直接决定架构里「带宽限制」是必需机制还是可选优化。

## 为什么自己写调度器

用 FreeRTOS / Zephyr 测出来的数字是**那个内核的**干扰量，
换一个内核就不成立。只有自研的最小实现才能把干扰归因清楚。

本实现刻意只保留三样东西：两个固定优先级、SysTick 1kHz 抢占、PendSV 上下文切换。
没有时间片、没有阻塞原语、没有内存管理。

## 硬件与工具

| 项 | 要求 |
|---|---|
| 开发板 | STM32H723 核心板 |
| 调试器 | **DAPLink（CMSIS-DAP）或 ST-Link 均可**，能跑 probe-rs 即可 |
| 软件 | Rust ≥ 1.88（需要 `#[unsafe(naked)]`）、probe-rs、`thumbv7em-none-eabihf` 目标 |

**不需要屏幕、不需要 USB、不需要串口线。** 输出走 RTT，也就是烧录用的那根 SWD 线。

### 探针接线（DAPLink / ST-Link 通用，4 根必需 + 1 根建议）

| 探针 | → | H723 | 说明 |
|---|---|---|---|
| **VTref** | → | **3.3V** | **必须接**。只做电平参考，不供电。悬空会报 `No target connected` |
| SWDIO | ↔ | PA13 | 双向 |
| SWCLK | → | PA14 | |
| GND | ↔ | GND | 共地 |
| nRESET | → | NRST | 建议接，保证烧完能自动复位运行 |

**不要双供电**：核心板已有独立电源时，VTref 只接 3.3V 做检测，
不要再把探针的 3.3V 当电源灌进去（会电流倒灌）。

**RTT 不需要 SWO 引脚** —— RTT 是通过 SWD 读目标内存实现的，
廉价 DAPLink 没有 SWO 也能用。

### 探针排障

| 现象 | 原因 |
|---|---|
| `probe-rs list` 为空 | VTref 没接，或 GND 没共地 |
| `No target connected` | VTref 悬空 / 目标未上电 |
| `multiple probes found` | 同时插了多个探针，需加 `--probe` |
| `parity error` / 连接不稳 | SWD 时钟太快，加 `--speed 1000` |
| 能烧录但看不到 RTT 输出 | `--chip` 型号错，导致 RTT 控制块地址找错 |

### 常见探针 VID:PID

| 探针 | 参数 |
|---|---|
| DAPLink（ARM mbed 官方） | `0d28:0204` |
| ST-Link V2 | `0483:3748` |
| ST-Link V2-1 / V3 | `0483:374b` / `0483:374e` |

用 `probe-rs list` 看到实际值最准。

## 一次性准备

```bash
rustup target add thumbv7em-none-eabihf
cargo install probe-rs-tools
```

**确认芯片型号**（`.cargo/config.toml` 里的 runner 行）：

```bash
probe-rs chip list | grep -i h723
```

常见：
- `STM32H723ZGTx` — 正点原子 H723 核心板多为 ZGT6
- `STM32H723VGTx` — LQFP100 封装

把 `.cargo/config.toml` 里的 `--chip STM32H723ZGTx` 改成实际型号。**改错的表现是烧录时找不到芯片或内存映射错乱。**

## 跑实验

```bash
# 1. 基线（必须最先跑）
#    把 src/main.rs 的 MODE 改成 load::Mode::Idle
cargo run --release > out-idle.csv

# 2. 满载 CPU
#    MODE 改成 load::Mode::Spin
cargo run --release > out-spin.csv

# 3. 模拟 Web
#    MODE 改成 load::Mode::Web
cargo run --release > out-web.csv

# 分析
python analysis/parse.py out-idle.csv
```

采样满 8192 条（约 8.2 秒）后自动停 SysTick、打印全部样本、停机。

## 代码结构

| 文件 | 职责 |
|---|---|
| `src/context.rs` | **naked 上下文切换**。PendSV 的 9 条汇编，无编译器插入 |
| `src/sched.rs` | 双优先级调度器、SysTick 释放逻辑、中断优先级配置 |
| `src/time.rs` | DWT CYCCNT 封装 |
| `src/load.rs` | 三种负载生成器（Idle / Spin / Web） |
| `src/measure.rs` | 采样缓冲与 CSV 输出 |
| `src/main.rs` | 入口，**改 `MODE` 常量切换实验点** |

## 关键设计决定（每条都有理由）

### 1. 运行在复位默认的 64MHz HSI，不配 PLL

PLL 配错会直接挂死且没有任何输出，调试成本极高。而本实验测的是**相对差异**
（有负载 vs 无负载），64MHz 还是 550MHz 不改变结论方向。

若后续要绝对数字，再补 PLL 配置，并同步修改 `time::CPU_HZ`。

### 2. 全程不用 FPU

一旦某个任务用了浮点指令，硬件会做 lazy stacking，异常帧多出 S0-S15 + FPSCR，
`EXC_RETURN` 要从 `0xFFFFFFFD` 改成 `0xFFFFFFE9`，栈布局全部要重算。
为了让上下文切换的假设保持简单，本固件只用整数运算。

### 3. 核心指标是 `t_start - t_release`，不是执行时间

执行时间只说明「任务跑了多久」，完全看不出「它被拖了多久」。
这是测实时性最常见的错误 —— 数字很漂亮，系统其实已经超期了。

### 4. PendSV 优先级必须最低

`SHPR3` 的 bit23-16 设为 0xFF。复位后所有异常优先级都是 0（最高），
不显式改的话上下文切换可能打断中断处理，任务栈会被踩。

### 5. 恢复点紧贴时间戳

`rt_task` 的循环体从 `yield_now()` 开始，所以任务恢复后的第一条指令就是读 `now()`。
任何夹在中间的判断（`while !released`）都会计入测量误差。

## 已验证（2026-11-21）

- `cargo build --release` 通过（rustc 1.97.1）
- PendSV 反汇编确认 **9 条指令，无编译器插入的序言/尾声**：
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
- 向量表偏移 0x38 → `0x08000459`（PendSV）、0x3C → `0x08000477`（SysTick）
- 初始 SP = `0x24050000`（RAM 末尾），`.bss` 74KB / 320KB
- **上板跑通：三个模式各 8192 样本全部采集成功，overruns=0**

### 实测结果（64 MHz，未配 PLL，未开 cache）

| 模式 | p50 | p99.9 | max（稳态） | mean |
|---|---|---|---|---|
| Idle | 229 cyc / 3.58 µs | 234 | 234 | 231.5 |
| Spin | 226 cyc / 3.53 µs | 226 | 235 | 226.0 |
| Web | 234 cyc / 3.66 µs | 234 | 234 | 232.3 |

- **负载类型几乎不影响调度延迟**（差异 ≤ 2.8%）
- **p50 = p99.9 = max，零长尾**
- 首次调度 415–431 周期是冷启动伪影（I-Cache 空），已从统计中排除
- 周期抖动峰峰值约 220 周期 = 3.5 µs

判读与实验局限见 `docs/EXPERIMENT-01.md` 第七节。

## 上板踩过的四个坑（都会静默失败，值得记住）

### 坑 1：SHPR3 位段写反 → 首次中断即 HardFault

`SHPR3`（`0xE000ED20`）的布局：

| 位段 | 异常 |
|---|---|
| `bits[23:16]` | **PendSV**（异常 14） |
| `bits[31:24]` | **SysTick**（异常 15） |

写反的后果不是"性能略差"，而是**直接崩**：

1. `yield_now()` 在 SysTick 里置 PendSV pending
2. PendSV 优先级变成最高 → **立刻抢占 SysTick**，形成嵌套异常
3. 入栈的是 Handler 模式现场（在 MSP 上），而 naked 处理器只按 **PSP** 保存 r4-r11
4. 现场全错 → `bx lr` 拿到 `0xFFFFFFF1`（返回 Handler 模式）→ **INVPC**

**识别指纹**：保存的上下文第一个字是 `0xFFFFFFF1` 而非 `0xFFFFFFFD`，
且保存的 `r7` 紧贴 MSP 顶（`0x2404FFB8` 一类地址）。

**取证路径**：读 `CFSR`（`0xE000ED28`）→ UFSR bit2 = INVPC；
再读 `SHPR3` 看实际优先级；最后 dump 任务保存的完整上下文。

### 坑 2：`wfi` 会让 SWD 调试口失联

`DBGMCU_CR.DBG_SLEEP`（bit 0）复位默认 0，核心一进 WFI 调试时钟就停：

```
WARN  Could not clear all hardware breakpoints: ... FAULT response
Error: An ARM specific error occurred.
```

随后连 `probe-rs read` 都读不到，必须 `probe-rs reset --connect-under-reset` 才能恢复。

已尝试置位 `DBGMCU_CR`（`0x5C001004` |= 1），**未能解决**，原因待查
（可能是 H7 的 D3 域调试时钟另有开关）。

⇒ Idle 模式改用 8×nop 空转。附带好处：排除 WFI 唤醒延迟，三种模式同口径可比。

> **DBGMCU 基址**：H7 是 `0x5C001000`，**不是** F4/F7 的 `0xE0042000`
> （后者在这块芯片上读出全 0）。

### 坑 3：RTT 默认模式静默丢包

`rtt_init_print!()` 默认是 **`NoBlockSkip`** —— 主机搬不过来就丢。
实测 8192 条样本只出来约 5690 条，**且没有任何报错**。

修法：`rtt_init_print!(rtt_target::ChannelMode::BlockIfFull, 4096)`

- 宏的形参是 `$mode:path`，不能只写 `BlockIfFull`
- 阻塞只发生在采样结束后的输出阶段，不污染测量值

### 坑 4：probe-rs 给 RTT 输出加时间戳前缀

实际输出是 `21:01:17.115: 0,431,0`，不是 `0,431,0`。
`parse.py` 必须先用正则剥掉 `^\d{2}:\d{2}:\d{2}\.\d+:\s*`，
否则 `int()` 全部抛异常、样本被当噪声静默跳过。


## 首次上板排查清单

| 现象 | 最可能的原因 |
|---|---|
| 完全无输出 | RTT 控制块没被探测到；确认 `--chip` 正确、`cargo run` 走的是 probe-rs |
| 有启动信息但无样本 | SysTick 没使能，或 `SHPR3` 优先级没设对导致 PendSV 不触发 |
| 立刻 HardFault | 栈顶未 8 字节对齐；或 `EXC_RETURN` 用错（用了 0xFFFFFFE9） |
| 样本全是 0 | `DEMCR.TRCENA` 没置位，CYCCNT 不计数 |
| 样本数远少于 8192 | 发生 overrun，看输出里的 `overruns=` 计数 |

## 这个实验不回答什么

- **不测内存隔离的正确性**：MPU 分区要靠故障注入测，不是时序测量。
- **不测 WCET 的 sound 性**：DWT 测的是执行时间分布，不是最坏情况上界。
- **不测总线竞争的真实影响**：`Web` 模式是软件模拟，不等于真实 SPI/网口流量。
- **不测调度器不变式**：那是 Verus 的活。
- **单核结论**：多核下 cache/bus 争用模型不同。

## 下一步

跑完三个模式后，把三份 CSV 的 `max` 摆在一起：

| 结果 | 架构含义 |
|---|---|
| 无限制干扰 < 周期 1% | 带宽限制降级为防御性措施，重心转向准入控制 |
| 无限制干扰 > 周期 10% | 带宽限制升为架构基石，`budget` 字段必须编译期校验 |
| 介于两者之间 | 需要按临界性分级：实时面独占、管理面共享、插件面受限 |

---

## P1：串口 CLI 与设备 API

Plan 9 风格命名、非阻塞语义、不用分配器。

### 设备

| 路径 | 权限 | 行为 |
|---|---|---|
| `/stat/sched` | 读 | `cpu_hz`、`overruns`、`tick_missed`、`samples` |
| `/stat/measure` | 读 | 缓冲填充、档位数、每档样本数 |
| `/stat/crit` | 读 | 运行期实测的中断屏蔽预算 |
| `/dev/log` | 读写 | 8 行环形日志；**满了丢最旧的，永不阻塞** |
| `/dev/led` | 写 | 板载蓝灯 PG7；`on` / `off` / `toggle` |

### 命令

```
ls
cat <path>
echo <text> > <path>
help
```

读处理器返回 `Result<(), ()>`，`Err` 打印 `(not ready)` 而不是假装成功。
这就是计划里的非阻塞规则 —— 用类型签名表达，而不是写一句注释。

### CLI 跑在哪里

**LOAD 任务里**，也就是最低优先级，这是刻意的。控制台属于不可信负载：
它慢、可能阻塞在 I/O 上，而且绝不能碰到实时面。

实测：15 条脚本命令连续执行期间，`overruns = 0`、`tick_missed = 0`。实时任务毫无察觉。

### 控制台是可替换的抽象，这不是装饰

`cli::out()` 通过 `Console` 枚举输出，两个后端：`Rtt` 与 `Uart`。RTT 后端存在的原因是
**手上没有 USB-TTL**，串口那条路现在没法验证。逻辑已通过 RTT 端到端验证；
UART 后端代码写好但**未上机验证** —— USART1 与 SWD 共用那个 2×4 排针，需要单独接三根线。

两件事都如实说明、不含糊。下面这段回放是板子上的真实输出：

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

`/dev/led` 的写入对着硬件核过：`on` 再 `toggle` 之后，`GPIOG.ODR` 的 bit7 读回 0。
命令确实动了引脚。

### 刻意没做的部分

计划里的 `/dev/actuator/*/ctl` 与 `/ctl/sched/admit` **没有实现**。
"每次写入都是一个提交请求、由实时面执行后异步确认"正是 P1 的核心，
用一次直接的 `ODR` 写入去冒充它，比不做更糟。
`/dev/led` 对自己的定位是诚实的：它是状态指示灯，不是带控制环的执行器。
