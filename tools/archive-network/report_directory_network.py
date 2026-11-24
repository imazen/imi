#!/usr/bin/env python3
"""Audit the bounded directory/CDN study; preserve every timing and slow control."""
import argparse
import csv
import json
from pathlib import Path
import xxhash

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('root', type=Path)
p.add_argument('output', type=Path)
p.add_argument('table', type=Path)
a = p.parse_args()
rows = []
audits = {}

for phase, expected in [('index', 6), ('layout', 42), ('native-replay', 24), ('pages', 12), ('cache', 4)]:
    directory = a.root / phase
    result = json.loads((directory / 'results.json').read_text())
    records = result['results']
    assert len(records) == expected, (phase, len(records))
    assert len({(r['label'], r['trial'], r['repeat']) for r in records}) == expected
    config = result['config']
    jobs = {j['label']: j for j in config['jobs']}
    for r in records:
        assert 'error' not in r
        job = jobs[r['label']]
        requests = job.get('requests', [job])
        assert len(r['network']) == len(requests), (phase, r['label'], 'CDP response count')
        assert all(n.get('status') in (200, 206) and not n.get('failed') for n in r['network'])
        identity = ''
        if phase == 'index':
            assert r['bytes'] == 3350452 and r['parsed']['entries'] == 76146
        else:
            data = (directory / r['bodyFile']).read_bytes()
            assert len(data) == r['bytes']
            identity = xxhash.xxh3_128_hexdigest(data)
            if phase == 'pages':
                index = (a.root / 'index/bigpine-full-index.bin').read_bytes()
                expected_bytes = b''
                for q in requests:
                    start, stop = map(int, q['range'][6:].split('-'))
                    expected_bytes += index[start:stop+1]
                assert data == expected_bytes
        headers = [{k.lower(): v for k, v in n.get('headers', {}).items()} for n in r['network']]
        rows.append(dict(phase=phase, label=r['label'], trial=r['trial'], repeat=r['repeat'],
                         requests=len(requests), dependency_stages=len({q.get('stage', 0) for q in requests}),
                         body_bytes=r['bytes'], non_disk_cache_responses=sum(not n.get('fromDiskCache') for n in r['network']),
                         disk_cache_responses=sum(bool(n.get('fromDiskCache')) for n in r['network']),
                         cdp_encoded_bytes=sum(n['encodedDataLength'] for n in r['network'])
                         if all('encodedDataLength' in n for n in r['network']) else '',
                         fetch_ms=r['fetchMs'], protocols=','.join(sorted({n['protocol'] for n in r['network']})),
                         cache_statuses=','.join('browser-cache' if n.get('fromDiskCache') else h.get('cf-cache-status', '?')
                                                for n, h in zip(r['network'], headers)), body_xxh3_128=identity))
    if phase in ('layout', 'native-replay', 'cache'):
        pairs = [('viewport-mono', 'viewport-split')] if phase == 'cache' else [
                 ('prefix-mono', 'prefix-split'), ('boundary-mono', 'boundary-split-parallel'),
                 ('boundary-mono', 'boundary-split-serial'), ('viewport-mono', 'viewport-split')] if phase == 'layout' else [
                     ('native-middle-mono', 'native-middle-split'),
                     ('native-middle-known-size-mono', 'native-middle-known-size-split')]
        for left, right in pairs:
            for trial in range(config['trials']):
                for repeat in range(config['repeats']):
                    assert (directory / f'{left}-{trial}-{repeat}.bin').read_bytes() == (
                        directory / f'{right}-{trial}-{repeat}.bin').read_bytes(), (left, right, trial, repeat)
        if phase == 'native-replay':
            for kind in ['mono', 'split']:
                for trial in range(config['trials']):
                    for repeat in range(config['repeats']):
                        baseline = (directory / f'native-middle-{kind}-{trial}-{repeat}.bin').read_bytes()
                        known = (directory / f'native-middle-known-size-{kind}-{trial}-{repeat}.bin').read_bytes()
                        assert baseline[1:] == known
    audits[phase] = dict(successful_rows=expected, identity_checks='passed')
    if phase == 'cache':
        for r in records:
            if r['label'] == 'viewport-split' and r['repeat'] == 1:
                assert all(n['fromDiskCache'] and n['encodedDataLength'] == 0 and n.get('wireStatus') is None for n in r['network'])
            else:
                assert all(not n['fromDiskCache'] and not n['servedFromCache'] and n['wireStatus'] == 206 and n['encodedDataLength'] > 0 for n in r['network'])

native = json.loads((a.root / 'native-v2/RESULTS.json').read_text())
assert len(native) == 9 and json.loads((a.root / 'native-v2/STATUS.json').read_text())['stage'] == 'complete'
for key, count in [(0, 5), (38073, 5), (76145, 2)]:
    selected = [r['result'] for r in native if r['result']['key'] == key]
    assert len(selected) == 3 and all(r['requests'] == count for r in selected)
    assert len({r['pixel_sha256'] for r in selected}) == 1  # Existing sidecar oracle.

with a.table.open('x', newline='') as f:
    w = csv.DictWriter(f, fieldnames=list(rows[0]), delimiter='\t')
    w.writeheader()
    w.writerows(rows)
summary = dict(scope='Real public CDN range observations, not a controlled pure-layout/H3/CPU benchmark.',
               audits=audits, native_source_pixel_checks=9,
               protocols=sorted({r['protocols'] for r in rows}),
               quic_target='Forced data-origin fetch failed; page-origin catalog negotiated h3. No data-origin H3 timing.',
               table_rows=len(rows))
a.output.write_text(json.dumps(summary, indent=2) + '\n')
print(json.dumps(summary))
