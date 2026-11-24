#!/usr/bin/env python3
"""summarize_off.py MATRIX.tsv: regular-map files only, arms vs 'base' (main).

Worst process median per (arm, file, threads) over rounds; per-set aggregate
time ratios arm/base at 1/2/4/8 threads; per-cell geomeans; cells slower than
base beyond the per-cell A/A (base2/base) difference.
"""
import collections, statistics, sys
rows = [l.rstrip('\n').split('\t') for l in open(sys.argv[1])]
assert rows[-1] == ['DONE'], 'incomplete study'
d = collections.defaultdict(dict)
for r, f, m, a, _, t, px, ms, rate in rows[:-1]:
    assert m == 'off'
    d[a, f, int(t)][int(r)] = float(ms)
rounds = {len(v) for v in d.values()}
w = {k: max(v.values()) for k, v in d.items()}
arms = sorted({k[0] for k in w}); others = [a for a in arms if a != 'base']
files = sorted({k[1] for k in w}); sets = sorted({f.split('/')[0] for f in files})
print('arms', arms, 'files', len(files), 'rounds per cell', rounds)
print('aggregate time arm/base, threads 1 / 2 / 4 / 8')
for s in sets + ['ALL']:
    ids = [f for f in files if s == 'ALL' or f.split('/')[0] == s]
    for a in others:
        rs = [sum(w[a, f, t] for f in ids) / sum(w['base', f, t] for f in ids) for t in [1, 2, 4, 8]]
        print(f'{s:14s} {a:10s}', ' / '.join(f'{x:.4f}' for x in rs))
print('per-cell geomean time arm/base and cells slower beyond per-cell A/A')
for a in others:
    keys = [(f, t) for f in files for t in [1, 2, 4, 8]]
    ab = [w[a, f, t] / w['base', f, t] for f, t in keys]
    aa = [w['base2', f, t] / w['base', f, t] for f, t in keys]
    beyond = sum(1 for x, y in zip(ab, aa) if x > 1 + abs(y - 1))
    print(f'{a:10s} geomean {statistics.geometric_mean(ab):.5f} beyond {beyond}/{len(keys)} worst {max(ab):.4f}')
