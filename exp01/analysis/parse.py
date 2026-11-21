#!/usr/bin/env python3
"""实验 01 结果分析。

用法：
    cargo run --release > out.csv        # probe-rs 的 RTT 输出直接重定向
    python analysis/parse.py out.csv

为什么百分位在 PC 上算而不是在 MCU 上算：
  8192 个样本排序在 64MHz 的 M7 上要几十毫秒，而且会挤占 RAM。
  更重要的是——排序算法本身会引入实现细节（是否原地、是否稳定），
  放在 PC 上算可以随时换统计方法，不用重新烧录。
"""

import sys
import statistics


def load(path):
    lat, per = [], []
    meta = []
    for line in open(path, encoding="utf-8", errors="replace"):
        line = line.strip()
        if not line:
            continue
        if line.startswith("#"):
            meta.append(line)
            continue
        parts = line.split(",")
        if len(parts) != 3:
            continue
        try:
            idx, l, p = int(parts[0]), int(parts[1]), int(parts[2])
        except ValueError:
            continue  # 跳过 probe-rs 自己的状态行
        lat.append(l)
        # 第一条没有前值，跳过
        if idx > 0:
            per.append(p)
    return lat, per, meta


def pct(sorted_vals, q):
    """最近秩法取分位数。样本量足够大时与插值法差异可忽略。"""
    if not sorted_vals:
        return 0
    k = max(0, min(len(sorted_vals) - 1, int(round(q / 100.0 * len(sorted_vals))) - 1))
    return sorted_vals[k]


def report(name, vals, cpu_hz):
    if not vals:
        print(f"{name}: 无样本")
        return
    s = sorted(vals)
    us = lambda c: c * 1e6 / cpu_hz
    print(f"\n{name}  n={len(s)}")
    print(f"  min      {min(s):>8} cyc  ({us(min(s)):>8.2f} us)")
    print(f"  p50      {pct(s, 50):>8} cyc  ({us(pct(s, 50)):>8.2f} us)")
    print(f"  p99      {pct(s, 99):>8} cyc  ({us(pct(s, 99)):>8.2f} us)")
    print(f"  p99.9    {pct(s, 99.9):>8} cyc  ({us(pct(s, 99.9)):>8.2f} us)")
    print(f"  max      {max(s):>8} cyc  ({us(max(s)):>8.2f} us)")
    print(f"  mean     {statistics.mean(s):>8.1f} cyc")


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        return 1

    # 固件当前跑在复位默认 64MHz。若改了 PLL，这里要同步。
    cpu_hz = 64_000_000

    lat, per, meta = load(sys.argv[1])
    for m in meta:
        print(m)

    report("调度延迟 latency (t_start - t_release)", lat, cpu_hz)
    report("周期抖动 period (t_start 相邻差)", per, cpu_hz)

    if lat:
        print("\n判读：")
        print("  latency 的 max 才是关键。p50 好看但 max 很大 = 尾部有长尾，")
        print("  对硬实时而言等同于超期。")
        print("  与基线（Mode::Idle）对比：若 max 增长超过实时任务周期的 10%，")
        print("  则带宽限制是架构必需项，而不是可选优化。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
