#!/usr/bin/env python3
"""lnoise-style medians, baseline relative rate ranges, plus worst-round controls."""
import collections, json, statistics, sys
p = sys.argv[1]
assert open(p).read().splitlines()[-1] == 'DONE', 'incomplete study'
d = collections.defaultdict(dict)
for l in open(p):
    if l.strip() == 'DONE':
        continue
    f = l.strip().split('\t')
    assert len(f) == 9 and f[2] == 'off', f
    r, fid, mode, arm, path, t, px, ms, speed = f
    k = (arm, fid, int(t))
    assert int(r) not in d[k]
    d[k][int(r)] = float(ms)
assert all((set(v) == set(range(1, 25)) for v in d.values())), 'expected rounds 1..24'
assert {k[0] for k in d} == {'mainref', 'main-11', 'main-29', 'main-47', 'adopted-11', 'adopted-29', 'adopted-47'}, 'unexpected arms'
seeds = [11, 29, 47]
cells = sorted({(f, t) for a, f, t in d})
assert len(cells) == 160
median = {k: statistics.median(v.values()) for k, v in d.items()}
worst = {k: max(v.values()) for k, v in d.items()}

def rate_spread(k):
    rates = [1 / v for v in d[k].values()]
    return (max(rates) - min(rates)) / statistics.median(rates)

def gmean(xs):
    return statistics.geometric_mean(xs)
report = {}
for metric, name in [(median, 'median'), (worst, 'worst')]:
    ab = {s: {c: metric[f'adopted-{s}', *c] / metric[f'main-{s}', *c] for c in cells} for s in seeds}
    aa = {s: {c: metric[f'main-{s}', *c] / metric['mainref', *c] for c in cells} for s in seeds}
    abgeo = {s: gmean(ab[s].values()) for s in seeds}
    aageo = {s: gmean(aa[s].values()) for s in seeds}
    overallab = gmean(abgeo.values())
    overallaa = gmean(aageo.values())
    print(name, 'AB time geomeans', abgeo, 'overall', overallab)
    print(name, 'AA time geomeans', aageo, 'overall', overallaa)
    print(name, 'geomean gate', overallab <= 1 or abs(overallab - 1) <= abs(overallaa - 1))
    badab = {s: {c for c in cells if 1 / ab[s][c] < 1 - rate_spread((f'main-{s}', *c))} for s in seeds}
    badaa = {s: {c for c in cells if 1 / aa[s][c] < 1 - rate_spread(('mainref', *c))} for s in seeds}
    consistent = set.intersection(*(badab[s] for s in seeds))
    slowerall = {c for c in cells if all((ab[s][c] > 1 for s in seeds))}
    print(name, 'AB slower-than-spread counts', {s: len(badab[s]) for s in seeds}, 'mean', statistics.mean((len(v) for v in badab.values())))
    print(name, 'AA slower-than-spread counts', {s: len(badaa[s]) for s in seeds}, 'mean', statistics.mean((len(v) for v in badaa.values())))
    print(name, 'consistent slower beyond spread', sorted(consistent))
    for c in sorted(consistent):
        print('CONSISTENT', name, c, 'ratios', [ab[s][c] for s in seeds], 'baseline spreads', [rate_spread((f'main-{s}', *c)) for s in seeds], 'AA ratios', [aa[s][c] for s in seeds])
    print(name, 'slower in every layout (including within spread)', sorted(slowerall))
    for t in [1, 2, 4, 8]:
        keys = [c for c in cells if c[1] == t]
        print(name, 'thread', t, 'AB', gmean((ab[s][c] for s in seeds for c in keys)), 'AA', gmean((aa[s][c] for s in seeds for c in keys)))
    report[name] = {'ab_per_layout': abgeo, 'aa_per_layout': aageo, 'ab_geomean': overallab, 'aa_geomean': overallaa, 'consistent_beyond_spread': sorted(consistent), 'slower_all_layouts': sorted(slowerall), 'ab_flagged_counts': {s: len(v) for s, v in badab.items()}, 'aa_flagged_counts': {s: len(v) for s, v in badaa.items()}}
with open(p + '.summary.json', 'x') as out:
    json.dump(report, out, indent=2)
