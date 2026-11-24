#!/usr/bin/env python3
"""summarize_arm.py MATRIX.tsv ARM: table/off speed of one arm per set and thread
count (worst process median over rounds, aggregate pixels over aggregate time),
the slowest single file's ratio over all thread counts, and off MPx/s."""
import collections, sys
arm = sys.argv[2]
cells = collections.defaultdict(list)
for l in open(sys.argv[1]):
    p = l.rstrip('\n').split('\t')
    if len(p) < 9 or p[3] != arm: continue
    r, f, mode, a, path, t, px, ms, mps = p
    cells[f, mode, int(t)].append((float(ms), int(px)))
mode_on = 'table' if any(k[1] == 'table' for k in cells) else 'auto'
for d in sorted({f.split('/')[0] for f, _, _ in cells}):
    files = sorted({f for f, _, _ in cells if f.split('/')[0] == d})
    rs, offs, worst = [], [], 9
    for t in [1, 2, 4, 8]:
        tot = {m: [0.0, 0] for m in ['off', mode_on]}
        for f in files:
            r = {}
            for m in ['off', mode_on]:
                ms = max(x[0] for x in cells[f, m, t]); px = cells[f, m, t][0][1]
                tot[m][0] += ms; tot[m][1] += px; r[m] = px / ms
            worst = min(worst, r[mode_on] / r['off'])
        on = tot[mode_on][1] / tot[mode_on][0]; off = tot['off'][1] / tot['off'][0]
        rs.append(on / off); offs.append(off / 1000)
    print(f"{d:14s} n={len(files)} " + ' / '.join(f'{x:.3f}' for x in rs) + f"  worst {worst:.3f}  off MPx/s " + ' / '.join(f'{x:.0f}' for x in offs))
