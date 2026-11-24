#!/usr/bin/env python3
"""Compare two byte-gate lists (main.rs or sizes.rs output) cell by cell.

Every cell must decode to the same pixels on both sides (last column), and
the report sums the encoded-size change per group of cells. Usage:

    compare.py BASE.tsv TIP.tsv [--group COLS] [--markdown]

COLS: comma-separated 0-based columns that form a group (default: every
column but the image name, length and hashes). Exit status 1 when a decoded
hash differs or the cell sets differ.
"""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import sys


def read(path):
    cells = {}
    for line in open(path):
        f = line.rstrip("\n").split("\t")
        key = tuple(f[:-3])
        if key in cells:
            sys.exit(f"{path}: duplicate cell {key}")
        cells[key] = (int(f[-3]), f[-2], f[-1])
    return cells


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("base")
    ap.add_argument("tip")
    ap.add_argument("--group", help="0-based key columns that form a group")
    ap.add_argument("--markdown", action="store_true")
    ap.add_argument("--intermediate", help="an intermediate revision's list, for change attribution")
    ap.add_argument("--changed-cells", type=Path, help="write every changed cell to a new JSON file")
    a = ap.parse_args()
    base, tip = read(a.base), read(a.tip)
    intermediate = read(a.intermediate) if a.intermediate else None
    if base.keys() != tip.keys():
        print(f"cell sets differ: {len(base.keys() - tip.keys())} only in base, "
              f"{len(tip.keys() - base.keys())} only in tip")
        return 1
    if intermediate is not None and base.keys() != intermediate.keys():
        print("intermediate cell set differs")
        return 1
    bad = [k for k in base if base[k][2] != tip[k][2]
           or (intermediate is not None and base[k][2] != intermediate[k][2])]
    if a.changed_cells:
        def record(v):
            return dict(bytes=v[0], encoded_hash=v[1], decoded_hash=v[2])

        cells = []
        for k in sorted(base):
            if base[k] != tip[k]:
                cell = dict(key=k, baseline=record(base[k]), final=record(tip[k]))
                if intermediate is not None:
                    cell.update(intermediate=record(intermediate[k]),
                                earlier_changed=base[k][:2] != intermediate[k][:2],
                                later_changed=intermediate[k][:2] != tip[k][:2])
                cells.append(cell)
        report = dict(baseline_sha256=hashlib.sha256(Path(a.base).read_bytes()).hexdigest(),
                      final_sha256=hashlib.sha256(Path(a.tip).read_bytes()).hexdigest(),
                      cells=cells)
        if a.intermediate:
            report['intermediate_sha256'] = hashlib.sha256(Path(a.intermediate).read_bytes()).hexdigest()
        with a.changed_cells.open('x') as out:
            json.dump(report, out, indent=2)
            out.write('\n')
    width = len(next(iter(base)))
    cols = [int(c) for c in a.group.split(",")] if a.group else list(range(1, width))
    groups = {}
    for k in base:
        g = tuple(k[c] for c in cols)
        groups.setdefault(g, []).append((base[k][0], tip[k][0]))
    rows = []
    for g in sorted(groups):
        v = groups[g]
        b = sum(x for x, _ in v)
        t = sum(y for _, y in v)
        d = [y - x for x, y in v]
        rows.append((" ".join(g), len(v), b / len(v), t / len(v), statistics.median(d), min(d), max(d),
                     100 * (t - b) / b))
    if a.markdown:
        print("| cells | n | mean bytes before | mean bytes after | median delta | min | max | total % |")
        print("|---|---:|---:|---:|---:|---:|---:|---:|")
        for r in rows:
            print(f"| {r[0]} | {r[1]} | {r[2]:.1f} | {r[3]:.1f} | {r[4]:+g} | {r[5]:+d} | {r[6]:+d} | {r[7]:+.3f} |")
    else:
        for r in rows:
            print(f"{r[0]}\tn={r[1]}\tbefore={r[2]:.1f}\tafter={r[3]:.1f}\tmedian={r[4]:+g}\t"
                  f"min={r[5]:+d}\tmax={r[6]:+d}\ttotal={r[7]:+.3f}%")
    print(f"{len(base)} cells; decoded pixels differ in {len(bad)}")
    for k in bad[:20]:
        print("DECODE DIFFERS:", "\t".join(k))
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
