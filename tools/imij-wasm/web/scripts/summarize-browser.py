#!/usr/bin/env python3
"""Export complete zenbench browser pairs using worst-of-three cell times."""
import argparse
import hashlib
import json
import math
from collections import defaultdict
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('packet', type=Path)
parser.add_argument('output', type=Path)
args = parser.parse_args()
data = defaultdict(list)
for block in range(3):
    records = json.loads((args.packet / f'case-0-block-{block}.json').read_text())
    if sorted(r['arm'] for r in records) != [0, 1, 2]:
        raise ValueError('expected baseline, candidate and A/A')
    for record in records:
        if record['status'] != 'completed' or record['exit_code'] != 0:
            raise ValueError('incomplete measurement')
        path = args.packet / Path(record['output']).name / 'stdout.txt'
        expected = record['artifacts'][record['output'] + '/stdout.txt']
        if hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            raise ValueError('changed observation file')
        lines = [line.removeprefix('observations=') for line in path.read_text().splitlines() if line.startswith('observations=')]
        if len(lines) != 1:
            raise ValueError('expected one observation record')
        for row in json.loads(lines[0])['rows']:
            data[record['arm'], row['variant'], row['threads'], row['name']].append(row)

cells = sorted({key[1:] for key in data})
groups = defaultdict(list)
observations = []
for variant, threads, name in cells:
    arms = [data[a, variant, threads, name] for a in range(3)]
    if any(len(a) != 3 for a in arms):
        raise ValueError('missing cell observations')
    if len({r['hash'] for a in arms for r in a}) != 1:
        raise ValueError('pixel mismatch')
    for a in arms:
        if any(not math.isfinite(r['endToEndMs']) or r['endToEndMs'] <= 0 for r in a):
            raise ValueError('invalid elapsed time')
    times = [max(r['endToEndMs'] for r in a) for a in arms]
    pixels = arms[0][0]['width'] * arms[0][0]['height']
    cell = dict(variant=variant, threads=threads, name=name, worst_ms=times,
                candidate_ratio=times[1] / times[0], aa_ratio=times[2] / times[0],
                mpx_s=[pixels / t / 1000 for t in times], samples=arms)
    observations.append(cell)
    groups[variant, threads].append(cell)
summary = []
for (variant, threads), rows in sorted(groups.items()):
    geom = lambda field: math.exp(sum(math.log(r[field]) for r in rows) / len(rows))
    summary.append(dict(variant=variant, threads=threads, cells=len(rows),
                        candidate_ratio=geom('candidate_ratio'), aa_ratio=geom('aa_ratio'),
                        largest_candidate_ratio=max(r['candidate_ratio'] for r in rows)))
# Exclusive output: neither a new parser nor another run overwrites evidence.
with args.output.open('x') as out:
    json.dump(dict(summary=summary, cells=observations), out, indent=2)
    out.write('\n')
print(json.dumps(summary, indent=2))
