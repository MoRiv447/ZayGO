#!/usr/bin/env python3
"""实验 02 结果分析：屏蔽中断时长 → 释放延迟的敏感度。

用法：
    cargo run --release > out-sweep.csv
    python analysis/parse_sweep.py out-sweep.csv

输入列：stage,mask_cycles,idx,phase_cycles,dispatch_cycles

两个指标的分工：
  phase    —— 相对理想释放时刻的偏移。**这是本实验的核心指标**，
              因为它不随 ISR 被推迟而漂移。
  dispatch —— ISR 入口到任务开始。预期与屏蔽时长无关，
              用来证明"屏蔽伤害的是何时开始响应，不是切换本身"。
"""

import re
import sys
from collections import defaultdict

# probe-rs 会给每条 RTT 输出加时间戳前缀
TS_PREFIX = re.compile(r"^\d{2}:\d{2}:\d{2}\.\d+:\s*")


def load(path):
    stages = defaultdict(list)
    meta = []
    for raw in open(path, encoding="utf-8", errors="replace"):
        line = TS_PREFIX.sub("", raw.strip())
        if not line:
            continue
        if line.startswith("#"):
            meta.append(line)
            continue
        parts = line.split(",")
        if len(parts) != 5:
            continue
        try:
            st, mask, _idx, phase, dispatch = (int(x) for x in parts)
        except ValueError:
            continue
        stages[(st, mask)].append((phase, dispatch))
    return stages, meta


def pct(sorted_vals, q):
    if not sorted_vals:
        return 0
    k = max(0, min(len(sorted_vals) - 1, int(round(q / 100.0 * len(sorted_vals))) - 1))
    return sorted_vals[k]


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 1

    # 固件跑在复位默认的 64MHz HSI；改了 PLL 要同步这里
    cpu_hz = 64_000_000
    period = cpu_hz // 1000  # 1ms

    stages, meta = load(sys.argv[1])
    for m in meta:
        print(m)
    print()

    us = lambda c: c * 1e6 / cpu_hz

    print("屏蔽时长 → 释放延迟")
    print(f"{'mask':>7} {'n':>5} {'p50':>8} {'p99':>8} {'max':>8} "
          f"{'disp p50':>9} {'Δ vs M=0':>10} {'Δ/mask':>8}")
    print("-" * 78)

    baseline = None
    rows = []
    for key in sorted(stages):
        mask = key[1]
        phases = sorted(p[0] for p in stages[key])
        disps = sorted(p[1] for p in stages[key])
        p50 = phases[len(phases) // 2]
        if baseline is None:
            baseline = p50
        delta = p50 - baseline
        ratio = f"{delta / mask * 100:.1f}%" if mask else "—"
        print(f"{mask:>7} {len(phases):>5} {p50:>8} {pct(phases, 99):>8} "
              f"{phases[-1]:>8} {disps[len(disps) // 2]:>9} {delta:>10} {ratio:>8}")
        rows.append((mask, p50, phases[-1], disps[len(disps) // 2], delta))

    print()
    print("换算成时间（@64MHz）：")
    for mask, p50, mx, disp, delta in rows:
        print(f"  mask={mask:>6} → p50 {us(p50):>7.1f}us  max {us(mx):>7.1f}us  "
              f"dispatch {us(disp):>5.1f}us")

    print()
    print("判读要点：")
    print("  1. 除最后一档外，Δ/mask 应接近 100% —— 屏蔽多久，最坏延迟就涨多久，1:1。")
    print("  2. dispatch 应全程恒定。它恒定才说明延迟来自'何时开始响应'，")
    print("     而不是'切换本身变贵了'。")
    print("  3. 超过一个周期的那一档，phase 会铺满整个周期（p50 ≈ 半周期）：")
    print("     这不是延迟变大，而是**相位锁定被破坏了**——释放点已经和数据流无关。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
