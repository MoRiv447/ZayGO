//! CLI 的设备表与读写处理。
//!
//! # 设计要点
//!
//! 设备处理器拿到的是一个 `&mut dyn core::fmt::Write`，**直接往控制台写**，
//! 不构造中间字符串。这样：
//!
//! - no_std 下不需要分配器
//! - 输出边生成边发送，不会因为缓冲不够而截断
//! - 同一个处理器既能打到串口也能打到 RTT，取决于传进来的是谁
//!
//! # 为什么读接口是 `fn(&mut dyn Write)` 而不是返回 `&str`
//!
//! 因为 `/stat/*` 这类内容要现算（次数、周期数），没法是静态字符串。
//! 返回值一律是 `Result<(), ()>`：`Err` 表示设备当前没有数据，
//! CLI 会打印 "not ready" 而不是假装成功 —— 这是计划里
//! "非阻塞语义" 的直接体现。

use core::fmt::Write;

use crate::measure;
use crate::sched;
use crate::time;

pub struct Device {
    pub path: &'static str,
    /// 读处理器。`None` 表示只写。
    pub read: Option<fn(&mut dyn Write) -> Result<(), ()>>,
    /// 写处理器，第二个参数是提交的内容。`None` 表示只读。
    pub write: Option<fn(&str, &mut dyn Write) -> Result<(), ()>>,
    /// 一句话说明，`ls` 里会显示。
    pub note: &'static str,
}

/// 设备表。顺序即 `ls` 的输出顺序。
pub static DEVICES: &[Device] = &[
    Device {
        path: "/stat/sched",
        read: Some(read_sched),
        write: None,
        note: "实时面运行快照",
    },
    Device {
        path: "/stat/measure",
        read: Some(read_measure),
        write: None,
        note: "采样缓冲状态",
    },
    Device {
        path: "/stat/crit",
        read: Some(read_crit),
        write: None,
        note: "中断屏蔽预算实测值",
    },
    Device {
        path: "/dev/log",
        read: Some(read_log),
        write: Some(write_log),
        note: "环形日志；满了丢最旧的，永不阻塞",
    },
    Device {
        path: "/dev/led",
        read: None,
        write: Some(write_led),
        note: "板载蓝灯 PG7；写 on/off/toggle",
    },
];

pub fn find(path: &str) -> Option<&'static Device> {
    DEVICES.iter().find(|d| d.path == path)
}

// ---------------------------------------------------------------------------
// /stat/sched
// ---------------------------------------------------------------------------

fn read_sched(w: &mut dyn Write) -> Result<(), ()> {
    let _ = writeln!(
        w,
        "cpu_hz        {}",
        time::cpu_hz()
    );
    let _ = writeln!(
        w,
        "overruns      {}",
        sched::OVERRUN.load(core::sync::atomic::Ordering::Relaxed)
    );
    let _ = writeln!(
        w,
        "tick_missed   {}",
        sched::TICK_MISSED.load(core::sync::atomic::Ordering::Relaxed)
    );
    let _ = writeln!(w, "samples       {}", measure::len());
    Ok(())
}

// ---------------------------------------------------------------------------
// /stat/measure
// ---------------------------------------------------------------------------

fn read_measure(w: &mut dyn Write) -> Result<(), ()> {
    let _ = writeln!(w, "buffer        {}/{}", measure::len(), measure::CAP);
    let _ = writeln!(w, "stages        {}", measure::STAGES);
    let _ = writeln!(w, "per_stage     {}", measure::PER_STAGE);
    Ok(())
}

// ---------------------------------------------------------------------------
// /stat/crit
// ---------------------------------------------------------------------------

fn read_crit(w: &mut dyn Write) -> Result<(), ()> {
    let _ = writeln!(w, "declared_max  {} cycles", crate::crit::MAX_MASK_CYCLES);
    let _ = writeln!(w, "{:<20} {:>8} {:>10} {:>8}", "site", "budget", "observed", "viol");
    for i in 0..crate::crit::site_count() {
        let _ = writeln!(
            w,
            "{:<20} {:>8} {:>10} {:>8}",
            crate::crit::SITE_NAMES[i],
            crate::crit::SITE_BUDGETS[i],
            crate::crit::SITE_MAX[i].load(core::sync::atomic::Ordering::Relaxed),
            crate::crit::SITE_VIOLATIONS[i].load(core::sync::atomic::Ordering::Relaxed)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// /dev/log —— 定长行环形缓冲
// ---------------------------------------------------------------------------

const LOG_LINES: usize = 8;
const LOG_LINE_LEN: usize = 48;

static mut LOG: [[u8; LOG_LINE_LEN]; LOG_LINES] = [[0; LOG_LINE_LEN]; LOG_LINES];
static mut LOG_LEN: [u8; LOG_LINES] = [0; LOG_LINES];
static mut LOG_HEAD: usize = 0; // 下一个要写的位置
static mut LOG_COUNT: usize = 0; // 已写入的总条数（用于判断是否回绕）

fn write_log(text: &str, w: &mut dyn Write) -> Result<(), ()> {
    unsafe {
        let slot = LOG_HEAD;
        let n = text.len().min(LOG_LINE_LEN);
        LOG[slot][..n].copy_from_slice(&text.as_bytes()[..n]);
        LOG_LEN[slot] = n as u8;
        LOG_HEAD = (slot + 1) % LOG_LINES;
        LOG_COUNT += 1;
    }
    let _ = writeln!(w, "ok, buffered");
    Ok(())
}

fn read_log(w: &mut dyn Write) -> Result<(), ()> {
    unsafe {
        if LOG_COUNT == 0 {
            // 没数据时返回 Err，让 CLI 打 "not ready" 而不是空输出
            return Err(());
        }
        let total = LOG_COUNT.min(LOG_LINES);
        // 从最旧的一条开始
        let start = if LOG_COUNT < LOG_LINES {
            0
        } else {
            LOG_HEAD
        };
        for k in 0..total {
            let i = (start + k) % LOG_LINES;
            let n = LOG_LEN[i] as usize;
            let s = core::str::from_utf8(&LOG[i][..n]).unwrap_or("<invalid utf8>");
            let _ = writeln!(w, "{:>2}: {}", LOG_COUNT - total + k + 1, s);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// /dev/led —— 板载蓝灯 PG7
// ---------------------------------------------------------------------------

const GPIOG: usize = 0x5802_1800;
const RCC_AHB4ENR: usize = 0x5802_44E0;
const ODR: usize = 0x14;

fn write_led(cmd: &str, w: &mut dyn Write) -> Result<(), ()> {
    unsafe {
        // GPIOG 时钟（AHB4ENR bit 6）
        let r = RCC_AHB4ENR as *mut u32;
        r.write_volatile(r.read_volatile() | (1 << 6));

        let odr = (GPIOG + ODR) as *mut u32;
        let cur = odr.read_volatile();
        let bit = 1u32 << 7;

        let new = match cmd {
            "on" => cur | bit,
            "off" => cur & !bit,
            "toggle" => cur ^ bit,
            _ => {
                let _ = writeln!(w, "err: 只接受 on / off / toggle");
                return Ok(());
            }
        };
        odr.write_volatile(new);
    }
    let _ = writeln!(w, "ok");
    Ok(())
}
