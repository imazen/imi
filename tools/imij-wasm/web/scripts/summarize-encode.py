#!/usr/bin/env python3
"""Validate encode observations; summarize worst-of-three and sampled leaves."""
import argparse
import collections
import json
import math
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('study', type=Path)
p.add_argument('output', type=Path)
a = p.parse_args()
s = json.loads((a.study / 'measure/summary.json').read_text())
if not s['complete'] or s['cells'] != len(s['rows']) or s['cells'] != s['planned']:
    raise ValueError('incomplete study')
groups = collections.defaultdict(list)
for row in s['rows']:
    for key in ['nativeTimes', 'browserTimes']:
        if len(row[key]) != 3 or any(not math.isfinite(t) or t <= 0 for t in row[key]):
            raise ValueError('invalid timing observations')
    groups[row['category'], row['effort']].append(row)
summary = []
for (category, effort), rows in groups.items():
    px = sum(r['width'] * r['height'] for r in rows)
    native = sum(max(r['nativeTimes']) for r in rows)
    browser = sum(max(r['browserTimes']) for r in rows)
    summary.append(dict(category=category, effort=effort, inputs=len(rows), pixels=px,
                        nativeWorstSumMs=native, browserWorstSumMs=browser,
                        nativeMpxs=px / native / 1000, browserMpxs=px / browser / 1000,
                        bitsPerPixel=8 * sum(r['encodedBytes'] for r in rows) / px))
profiles = []
for path in sorted((a.study / 'profiles').glob('*.cpuprofile')):
    profile = json.loads(path.read_text())['profile']
    nodes = {n['id']: n['callFrame']['functionName'] for n in profile['nodes']}
    counts = collections.Counter(nodes[n] for n in profile['samples'])
    total = len(profile['samples'])
    entropy = sum(count for name, count in counts.items() if any(
        n in name for n in ['4rans19encode_lanes_tables', '4rans12lane_symbols', '4rans9lane_hist']))
    profiles.append(dict(image=path.stem, samples=total, entropySamples=entropy,
                         entropyPercent=entropy * 100 / total,
                         leaves=[dict(symbol=n, samples=c, percent=c * 100 / total)
                                 for n, c in counts.most_common()]))
a.output.write_text(json.dumps(dict(cells=summary, profiles=profiles), indent=2) + '\n')
for c in summary:
    print(f"| {c['category']} | {c['effort']} | {c['nativeMpxs']:.2f} | {c['browserMpxs']:.2f} | {c['bitsPerPixel']:.3f} |")
