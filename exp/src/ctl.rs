//! 控制请求队列：把"写入"和"生效"分开。
//!
//! # 这是 P1 的核心
//!
//! 计划里那条规则：
//!
//! > 每一次写入都是一个**提交的请求**，永远不是直接改状态。
//! > 状态变更由实时面执行并确认。
//!
//! 这个模块就是那条规则的实现。它是"Web 与插件不可预测"这件事被彻底封死的机制：
//! 无论上层多慢、多乱、多频繁，它能做的只有往队列里塞一条请求；
//! **什么时候生效由实时任务决定，不由它决定。**
//!
//! # 为什么入队是一段临界区
//!
//! 生产者是 LOAD 任务（不可信、最低优先级），消费者是实时任务（最高优先级）。
//! 后者会把前者在任何一条指令处打断，所以队头/队尾索引的更新必须互斥。
//!
//! 这段临界区用 `crit::mask` 包起来，因此**它会出现在 `/stat/crit` 的实测表里** ——
//! 不是白拿的：为了"提交"这个便利，实时面要付出几十个周期的屏蔽代价。
//! 这个代价是有账可查的，不是隐含的。
//!
//! # 满了怎么办
//!
//! **丢弃并返错，不阻塞。** 一个控制台请求因为队列满而卡住实时任务，
//! 是这个设计最不能接受的结果。宁可丢一条，也不让不可信侧拖住可信侧。

use core::sync::atomic::{AtomicU32, Ordering};

use crate::crit;
use crate::time;

/// 队列深度。取 8：足以吸收一次连发，又不至于让最坏屏蔽时长随深度增长。
pub const QUEUE_LEN: usize = 8;

/// 请求种类。
pub const KIND_LED: u8 = 1;

#[derive(Clone, Copy)]
struct Entry {
    kind: u8,
    arg: u32,
    seq: u32,
    /// 提交时刻（CYCCNT），用于算"提交到生效"的延迟。
    ts: u32,
}

const EMPTY: Entry = Entry {
    kind: 0,
    arg: 0,
    seq: 0,
    ts: 0,
};

static mut Q: [Entry; QUEUE_LEN] = [EMPTY; QUEUE_LEN];
static mut HEAD: usize = 0; // 生产者写入位置
static mut TAIL: usize = 0; // 消费者读取位置
static mut NEXT_SEQ: u32 = 1;

/// 已确认（已生效）的最大序号。
pub static ACKED: AtomicU32 = AtomicU32::new(0);
/// 因队列满而丢弃的数量。
pub static DROPPED: AtomicU32 = AtomicU32::new(0);
/// 最近一次"提交 → 生效"的延迟（周期）。
pub static LAST_LATENCY: AtomicU32 = AtomicU32::new(0);
/// 观察到的最大延迟（周期）。
pub static MAX_LATENCY: AtomicU32 = AtomicU32::new(0);

/// 当前待处理条数。
pub fn pending() -> u32 {
    unsafe {
        let (h, t) = (HEAD, TAIL);
        if h >= t {
            (h - t) as u32
        } else {
            (QUEUE_LEN - t + h) as u32
        }
    }
}

/// 提交一条请求。**立即返回**，不等待生效。
///
/// 返回 `Some(seq)` 表示已入队；`None` 表示队列满、已丢弃。
pub fn submit(kind: u8, arg: u32) -> Option<u32> {
    let mut seq = None;
    let ts = time::now();

    // 入队本身是一段真实的临界区：消费者（实时任务）会在任意指令处抢占。
    crit::mask(crit::SITE_CTL_SUBMIT, || unsafe {
        let h = HEAD;
        let next = (h + 1) % QUEUE_LEN;
        if next == TAIL {
            // 满：丢弃，绝不阻塞
            DROPPED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let s = NEXT_SEQ;
        NEXT_SEQ = NEXT_SEQ.wrapping_add(1);
        Q[h] = Entry { kind, arg, seq: s, ts };
        HEAD = next;
        seq = Some(s);
    });

    seq
}

/// 由实时任务在每个控制周期调用：把待处理请求全部执行掉。
///
/// 返回本次执行的条数。
///
/// 消费者是最高优先级任务，不会被生产者抢占，所以这里不需要再关中断 ——
/// 关掉反而是白付代价，那是"看起来安全"的写法。
pub fn apply_pending() -> u32 {
    let mut n = 0u32;
    unsafe {
        while TAIL != HEAD {
            let t = TAIL;
            let e = Q[t];
            TAIL = (t + 1) % QUEUE_LEN;

            match e.kind {
                KIND_LED => crate::dev::led_apply(e.arg),
                _ => {}
            }

            // 记录延迟并确认
            let lat = time::elapsed(e.ts);
            LAST_LATENCY.store(lat, Ordering::Relaxed);
            MAX_LATENCY.fetch_max(lat, Ordering::Relaxed);
            ACKED.store(e.seq, Ordering::Relaxed);
            n += 1;
        }
    }
    n
}
