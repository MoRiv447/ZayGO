//! 行编辑与命令分派。
//!
//! 与传输完全解耦：`feed()` 只接受字节、`execute()` 只接受一行文本，
//! 输出统一写进 `&mut dyn core::fmt::Write`。所以同一套逻辑既能跑在
//! USART1 上，也能跑在 RTT 上 —— 后者让没有串口线的环境也能验证行为。
//!
//! # 解析策略
//!
//! 手写解析，不引入依赖。语法只有三条：
//!
//! ```text
//! ls
//! cat <path>
//! echo <text> > <path>
//! ```
//!
//! 刻意**不接受** `>` 出现在文本中间以外的形式，也不做引号转义 ——
//! 声明式、可预测，比"看起来像 shell"更有用。

use core::fmt::Write;

use crate::dev;

pub const LINE_MAX: usize = 96;

pub struct Cli {
    buf: [u8; LINE_MAX],
    len: usize,
    /// 还没打过的提示符是否需要重画（上一次输出不以换行结尾时为真）
    pub dirty: bool,
}

impl Cli {
    pub const fn new() -> Self {
        Self {
            buf: [0; LINE_MAX],
            len: 0,
            dirty: false,
        }
    }

    /// 喂一个字节。返回 `Some(行)` 表示收到完整一行。
    pub fn feed(&mut self, b: u8, out: &mut dyn Write) -> Option<&str> {
        match b {
            // 回车与换行都当提交，且忽略随后紧跟的另一个
            b'\r' | b'\n' => {
                if self.len == 0 {
                    return None;
                }
                let n = self.len;
                self.len = 0;
                // SAFETY: buf[..n] 只包含从 &str 拆出的字节（见下方 push）
                return Some(unsafe { core::str::from_utf8_unchecked(&self.buf[..n]) });
            }
            // 退格
            0x08 | 0x7F => {
                if self.len > 0 {
                    self.len -= 1;
                    // 终端里退掉一个字符：退格 + 空格 + 退格
                    let _ = write!(out, "\x08 \x08");
                }
                None
            }
            // 可打印 ASCII
            0x20..=0x7E => {
                if self.len < LINE_MAX {
                    self.buf[self.len] = b;
                    self.len += 1;
                    // 回显，让串口终端有本地回显的效果
                    let _ = write!(out, "{}", b as char);
                }
                None
            }
            // 其余控制字符忽略
            _ => None,
        }
    }

    /// 重置行缓冲（例如收到 Ctrl-C）。
    pub fn clear_line(&mut self) {
        self.len = 0;
    }
}

/// 执行一行命令，把结果写进 `out`。
pub fn execute(line: &str, out: &mut dyn Write) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }

    let (cmd, rest) = split_first(line);

    match cmd {
        "help" | "?" => help(out),
        "ls" => ls(out),
        "cat" => {
            let path = rest.trim();
            if path.is_empty() {
                let _ = writeln!(out, "err: 用法 cat <path>");
                return;
            }
            match dev::find(path) {
                None => {
                    let _ = writeln!(out, "err: 没有这个设备: {}", path);
                }
                Some(d) => match d.read {
                    None => {
                        let _ = writeln!(out, "err: {} 是只写的", path);
                    }
                    Some(f) => match f(out) {
                        Ok(()) => {}
                        // 非阻塞语义：没数据就说没数据，不假装成功
                        Err(()) => {
                            let _ = writeln!(out, "(not ready)");
                        }
                    },
                },
            }
        }
        "echo" => {
            // 形式必须是 `echo <text> > <path>`；`>` 只在后半段找
            let rest = rest.trim();
            match rest.split_once('>') {
                None => {
                    let _ = writeln!(out, "err: 用法 echo <text> > <path>");
                }
                Some((text, path)) => {
                    let path = path.trim();
                    let text = text.trim();
                    if path.is_empty() {
                        let _ = writeln!(out, "err: 缺少目标路径");
                        return;
                    }
                    match dev::find(path) {
                        None => {
                            let _ = writeln!(out, "err: 没有这个设备: {}", path);
                        }
                        Some(d) => match d.write {
                            None => {
                                let _ = writeln!(out, "err: {} 是只读的", path);
                            }
                            Some(f) => {
                                let _ = f(text, out);
                            }
                        },
                    }
                }
            }
        }
        _ => {
            let _ = writeln!(out, "err: 未知命令 `{}`，输入 help 看用法", cmd);
        }
    }
}

fn help(out: &mut dyn Write) {
    let _ = writeln!(out, "命令：");
    let _ = writeln!(out, "  ls                     列出所有设备");
    let _ = writeln!(out, "  cat <path>             读一个设备");
    let _ = writeln!(out, "  echo <text> > <path>   向设备提交内容");
    let _ = writeln!(out, "  help                   这段说明");
    let _ = writeln!(out, "");
    let _ = writeln!(out, "所有写入都是「提交请求」，由实时面执行后确认；读取永不阻塞。");
}

fn ls(out: &mut dyn Write) {
    for d in dev::DEVICES {
        let rw = match (d.read.is_some(), d.write.is_some()) {
            (true, true) => "rw",
            (true, false) => "r-",
            (false, true) => "-w",
            (false, false) => "--",
        };
        let _ = writeln!(out, "{}  {:<20} {}", rw, d.path, d.note);
    }
}

fn split_first(s: &str) -> (&str, &str) {
    match s.find(' ') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s, ""),
    }
}

// ---------------------------------------------------------------------------
// 控制台后端与运行器
// ---------------------------------------------------------------------------

/// 输出目标。
///
/// `Rtt` 是**当前能验证的那个** —— 手上没有 USB-TTL，串口线插不上，
/// 所以行为验证走 RTT。`Uart` 是实机形态，代码已写好但未经实测，
/// 这一点在文档里如实标注，不假装验证过。
#[derive(Clone, Copy, PartialEq)]
pub enum Console {
    Rtt,
    Uart,
}

/// 当前控制台。
///
/// 改为 `Uart` 后跑在 USART1（PA9/PA10），经 DAPLink 内置的 USB-TTL
/// 出现在 COM16（`VID_0D28&PID_0204`，与探针同一 VID/PID）。
pub const CONSOLE: Console = Console::Uart;

/// 是否回放内置脚本。串口模式下关掉，以便真正测试交互输入。
const RUN_SCRIPT: bool = false;

struct RttWriter;
impl Write for RttWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        rtt_target::rprint!("{}", s);
        Ok(())
    }
}

struct UartWriter;
impl Write for UartWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // 必须复用 uart::Writer —— 它负责把换行展开成 CRLF。
        // 早先这里直接逐字节调 write_byte，少了回车，
        // 表现是空行消失、相邻两行粘在一起（终端只下移不回行首）。
        use core::fmt::Write as _;
        let mut w = crate::uart::Writer;
        w.write_str(s)
    }
}

/// 按当前配置输出。所有 CLI 输出都必须走这里，不能直接 `rprintln!`，
/// 否则切到 Uart 之后仍会往 RTT 写。
pub fn out<F: FnOnce(&mut dyn Write)>(f: F) {
    match CONSOLE {
        Console::Rtt => {
            let mut w = RttWriter;
            f(&mut w);
        }
        Console::Uart => {
            let mut w = UartWriter;
            f(&mut w);
        }
    }
}

/// 从输入源取一个字节。
fn read_byte() -> Option<u8> {
    match CONSOLE {
        // RTT 下行通道没有配置，所以 Rtt 模式下没有交互输入 ——
        // 交互验证改由脚本回放完成（见 [`demo_step`]）。
        Console::Rtt => None,
        Console::Uart => crate::uart::try_read(),
    }
}

static mut RUNNER: Option<Runner> = None;

struct Runner {
    cli: Cli,
    script_idx: usize,
}

/// 回放脚本。每一步执行一条命令，用来在没有串口线的情况下
/// 端到端验证解析、分派、设备读写、错误处理。
const SCRIPT: &[&str] = &[
    "help",
    "ls",
    "cat /stat/sched",
    "cat /stat/measure",
    "cat /stat/crit",
    "cat /dev/log",
    "echo first line > /dev/log",
    "echo second line > /dev/log",
    "cat /dev/log",
    "cat /nope",
    "echo x > /stat/sched",
    "badcmd arg",
    "echo on > /dev/led",
    "echo toggle > /dev/led",
    "echo nonsense > /dev/led",
];

/// 由 LOAD 任务每轮调用一次：先处理串口输入，再（若脚本未放完）回放一步。
///
/// 放在 LOAD 任务里是刻意的 —— 控制台是"不可信负载"，
/// 它慢、它可能阻塞，但它跑在最低优先级上，碰不到实时面。
pub fn poll() {
    unsafe {
        if RUNNER.is_none() {
            RUNNER = Some(Runner {
                cli: Cli::new(),
                script_idx: 0,
            });
            // 首次进入：打招呼并给出提示符。串口终端不会自己显示提示符，
            // 不打印的话用户面对的是空白。
            out(|w| {
                let _ = writeln!(w, "");
                let _ = writeln!(w, "zaygo CLI —— 输入 help 看用法");
                let _ = write!(w, "{}", PROMPT);
            });
        }
        let r = RUNNER.as_mut().unwrap();

        // 1) 串口输入（Uart 模式下才有数据）
        //    no_std 没有分配器，所以把行拷进栈上的定长缓冲再执行，
        //    不能持有 cli.buf 的借用去跨 out() 调用。
        let mut tmp = [0u8; LINE_MAX];
        while let Some(b) = read_byte() {
            let mut tn = 0usize;
            out(|w| {
                if let Some(s) = r.cli.feed(b, w) {
                    tn = s.len().min(LINE_MAX);
                    tmp[..tn].copy_from_slice(&s.as_bytes()[..tn]);
                }
            });
            if tn > 0 {
                let line = core::str::from_utf8(&tmp[..tn]).unwrap_or("");
                out(|w| {
                    let _ = writeln!(w);
                    execute(line, w);
                    let _ = write!(w, "{}", PROMPT);
                });
            }
        }

        // 2) 脚本回放（每次只走一条，把执行摊到多轮里，避免长时间占住 LOAD 任务）
        if RUN_SCRIPT && r.script_idx < SCRIPT.len() {
            let cmd = SCRIPT[r.script_idx];
            r.script_idx += 1;
            out(|w| {
                let _ = writeln!(w, "{}{}", PROMPT, cmd);
                execute(cmd, w);
            });
        }
    }
}

pub const PROMPT: &str = "zaygo> ";
