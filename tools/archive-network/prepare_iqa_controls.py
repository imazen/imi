#!/usr/bin/env python3
"""Freeze matched I01 cold/warm selection and summary-browsing controls.

Reuses the retained archive-network CONFIG-final.json and original source.tsv.
Grouped summary queries count groups, not per-distortion virtual entries; that
semantic gap must remain explicit rather than being called item browsing.
"""
import argparse
import json
from pathlib import Path
import xxhash

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('source_table', type=Path)
p.add_argument('retained_study', type=Path)
p.add_argument('client')
p.add_argument('output', type=Path)
a = p.parse_args()
assert not a.output.exists()
rows = [line.split('\t') for line in a.source_table.read_text().splitlines()]
assert len(rows) == 126 and rows[0][0] == 'I01.png'
config = json.loads((a.retained_study/'CONFIG-final.json').read_text())
config['client'] = a.client
config['profiles'] = config['profiles'][:2]
cases = []
for ordinal in [0, 1, 63, 125]:
    expected = rows[ordinal][4]  # Existing source-pixel protocol identity.
    assert len(expected) == 64
    for version, archive in [('v3', 'independent'), ('v4', 'independent-v4')]:
        cases.append(dict(name=f'{version}-independent-{ordinal}', archive=archive,
                          args=['--image', str(ordinal*2+1), expected]))
    for version, archive in [('v3', 'reference-v3-reindexed'), ('v4', 'reference-v4')]:
        for repeat in [False, True]:
            mode = '--indexed-repeat' if repeat else '--indexed'
            args = [mode, '1', str(ordinal), expected] + (['3'] if repeat else [])
            cases.append(dict(name=f'{version}-reference-{ordinal}'+('-repeat' if repeat else ''),
                              archive=archive, args=args))
for archive in ['independent', 'independent-v4', 'reference-v3-reindexed', 'reference-v4']:
    count = 126 if archive.startswith('independent') else 1
    for mode in ['pruned', 'scan']:
        cases.append(dict(name=f'{archive}-summary-{mode}', archive=archive,
                          args=['--dimensions', mode, '512', '384', str(count)]))
config['cases'] = cases
config['preparation'] = dict(source_table_xxh3_128=xxhash.xxh3_128(a.source_table.read_bytes()).hexdigest(),
                             coverage='I01 first/second/middle/last selection, reused grouped readers, dimension summary browsing; per-item attributes/ROI/append are not qualified here.')
a.output.write_text(json.dumps(config, indent=2)+'\n')
print(json.dumps(dict(cases=len(cases), cells=len(cases)*len(config['profiles'])*config['trials'])))
