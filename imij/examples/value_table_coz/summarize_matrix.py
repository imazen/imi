#!/usr/bin/env python3
import collections, json, statistics, sys
from pathlib import Path
p = Path(sys.argv[1])
assert p.read_text().splitlines()[-1] == 'DONE', 'incomplete study'
d = collections.defaultdict(dict)
for line in p.read_text().splitlines():
    if line == 'DONE':
        continue
    r, f, m, a, _, t, px, ms, rate = line.split('\t')
    k = (a, f, m, int(t))
    assert int(r) not in d[k]
    d[k][int(r)] = float(ms)
assert all((set(rounds) == {1, 2, 3} for rounds in d.values())), 'expected rounds 1..3'
assert {key[0] for key in d} == {'base', 'base2', 'adopted'}, 'unexpected arms'
w = {k: max(v.values()) for k, v in d.items()}
mode = 'table' if any((k[2] == 'table' for k in w)) else 'auto'
report = {'sets': {}, 'cells': []}
for s in sorted({f.split('/')[0] for a, f, m, t in w}):
    ts = {}
    for t in [1, 2, 4, 8]:
        ids = sorted({f for a, f, m, tt in w if f.split('/')[0] == s and tt == t})
        agg = lambda a, m: sum((w[a, f, m, t] for f in ids))
        ts[t] = {'table_off_speed': agg('adopted', 'off') / agg('adopted', mode), 'table_speed_base': agg('base', mode) / agg('adopted', mode), 'off_time_base': agg('adopted', 'off') / agg('base', 'off'), 'table_AA_time': agg('base2', mode) / agg('base', mode), 'off_AA_time': agg('base2', 'off') / agg('base', 'off')}
    report['sets'][s] = ts
    print(s)
    for field in next(iter(ts.values())):
        print(field, ' / '.join((f'{ts[t][field]:.6f}' for t in ts)))
for m in [mode, 'off']:
    rows = []
    for a, f, mm, t in w:
        if a != 'base' or mm != m:
            continue
        ab = w['adopted', f, m, t] / w['base', f, m, t]
        aa = w['base2', f, m, t] / w['base', f, m, t]
        rows.append({'file': f, 'threads': t, 'mode': m, 'time_base': ab, 'AA_time_base': aa, 'slower_beyond_cell_AA': ab > 1 + abs(aa - 1)})
    report['cells'] += rows
    geo = statistics.geometric_mean((r['time_base'] for r in rows))
    aageo = statistics.geometric_mean((r['AA_time_base'] for r in rows))
    print(m, 'time geomean', geo, 'AA geomean', aageo, 'slower beyond per-cell AA', sum((r['slower_beyond_cell_AA'] for r in rows)), '/', len(rows))
    for r in sorted(rows, key=lambda r: r['time_base'], reverse=True)[:8]:
        print('WORST', r)
with p.with_suffix('.analysis.json').open('x') as out:
    json.dump(report, out, indent=2)
