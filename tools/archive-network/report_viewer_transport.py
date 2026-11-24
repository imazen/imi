#!/usr/bin/env python3
"""Audit actual viewer-source ranges against the prior immutable CDN witnesses."""
import argparse
import csv
import json
from pathlib import Path
import xxhash

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('results', type=Path)
p.add_argument('prior', type=Path)
p.add_argument('table', type=Path)
a = p.parse_args()
d = json.loads((a.results / 'results.json').read_text())
rows = d['results']
assert len(rows) == 14
assert len({(r['label'], r['trial'], r['repeat']) for r in rows}) == 14
assert not d['config'].get('browserArgs')
out = []
for r in rows:
    assert 'error' not in r
    body = (a.results / r['bodyFile']).read_bytes()
    boundary = 'boundary' in r['label']
    expected = a.prior / 'layout' / ('boundary-mono-0-0.bin' if boundary else 'viewport-mono-0-0.bin')
    assert body == expected.read_bytes(), r['label']
    responses = [n for n in r['network'] if not n.get('failed')]
    assert all(n['status'] in (200, 204, 206) for n in responses)
    assert r['sourceCapacity'] >= len(body)
    gets = [n for n in responses if n['method'] == 'GET']
    heads = [n for n in responses if n['method'] == 'HEAD']
    options = [n for n in responses if n['method'] == 'OPTIONS']
    assert len(heads) == r['sourceHeadRequests']
    assert len(heads) + len(gets) == r['sourceRequests']
    if r['repeat'] == 1 and 'reopened' not in r['label']:
        assert not responses and r['sourceRequests'] == 0 and r['sourceFetchedBytes'] == 0
    out.append(dict(label=r['label'], repeat=r['repeat'], fetch_ms=r['fetchMs'],
                    source_requests=r['sourceRequests'], heads=len(heads), gets=len(gets), options=len(options),
                    disk_gets=sum(n['fromDiskCache'] for n in gets),
                    wire_gets=sum(n.get('wireStatus') == 206 for n in gets),
                    logical_bytes=len(body), fetched_bytes=r['sourceFetchedBytes'],
                    capacity=r['sourceCapacity'], peak_inflight=r['sourceStats']['peakInflight'],
                    validation='encoded-byte-equality',
                    body_xxh3_128=xxhash.xxh3_128_hexdigest(body)))
with a.table.open('x', newline='') as f:
    writer = csv.DictWriter(f, fieldnames=list(out[0]), delimiter='\t')
    writer.writeheader()
    writer.writerows(out)
prior_config = json.loads((a.prior / 'layout/CONFIG.json').read_text())
viewport = next(j for j in prior_config['jobs'] if j['label'] == 'viewport-mono')
planning = []
for mib in [8, 16, 32, 64, 128]:
    size = mib * 1024 ** 2
    parts = []
    for q in viewport['requests']:
        start, end = map(int, q['range'][6:].split('-'))
        parts.extend(range(start // size, end // size + 1))
    planning.append(dict(part_mib=mib, pieces=len(parts), urls=len(set(parts)),
                         max_per_url=max(parts.count(i) for i in set(parts))))
print(json.dumps(dict(rows=len(out), byte_checks='passed', retained_repeat_source_IO='zero',
                      exact_mapping_counts_only=planning,
                      scope='Actual deployed transport, fresh browser contexts, one witness per case; no full navigation/paint or part-size adoption.')))
