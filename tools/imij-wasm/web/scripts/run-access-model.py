"""Run a fixed transport-policy matrix; reject any decoded-output disagreement."""
import argparse
import csv
import hashlib
import itertools
import json
from pathlib import Path
import subprocess

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('binary', type=Path)
p.add_argument('trace', type=Path)
p.add_argument('output', type=Path)
p.add_argument('--rtts', type=float, nargs='+', default=[0, 160])
p.add_argument('--caches', type=int, nargs='+', default=[0, 65536, 33554432])
p.add_argument('--concurrencies', type=int, nargs='+', default=[1, 8])
p.add_argument('--cache-scope', choices=['object', 'shared'], default='object')
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=False)
reference = None
entry_reference = None
results = []
for delivery, mode, cache, rtt, concurrency in itertools.product(['serial', 'batch'], ['ranges', 'whole'], a.caches, a.rtts, a.concurrencies):
    name = f'{delivery}-{mode}-cache{cache}-rtt{rtt:g}-n{concurrency}'
    prefix = a.output / name
    command = [str(a.binary.resolve()), str(a.trace.resolve()), str(prefix.resolve()), mode,
               str(cache), str(rtt), '12500000', str(concurrency), delivery, a.cache_scope]
    (a.output / (name + '.command.json')).write_text(json.dumps(command) + '\n')
    with (a.output / (name + '.log')).open('x') as log:
        result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=False)
    (a.output / (name + '.rc')).write_text(str(result.returncode) + '\n')
    if result.returncode:
        raise RuntimeError(f'{name} failed; outputs preserved')
    with Path(str(prefix) + '.views.tsv').open() as f:
        rows = list(csv.DictReader(f, delimiter='\t'))
    if not rows:
        raise ValueError('empty replay')
    pixels = [(r['label'], r['decoded_bytes'], r['xxh3']) for r in rows]
    if reference is None:
        reference = pixels
    elif pixels != reference:
        raise ValueError(f'pixel/domain mismatch: {name}')
    with Path(str(prefix) + '.entries.tsv').open() as f:
        entries = list(csv.DictReader(f, delimiter='\t'))
    entry_pixels = [(r['label'], r['key'], r['decoded_bytes'], r['xxh3']) for r in entries]
    if entry_reference is None:
        entry_reference = entry_pixels
    elif entry_pixels != entry_reference:
        raise ValueError(f'entry pixel/domain mismatch: {name}')
    readiness = []
    for row in rows:
        cells = [e for e in entries if e['label'] == row['label']]
        times = [float(e['data_ready_ms']) for e in cells]
        if sum(int(e['decoded_bytes']) for e in cells) != int(row['decoded_bytes']):
            raise ValueError('entry/view decoded byte mismatch')
        if times != sorted(times) or any(t < float(row['start_ms']) or t > float(row['finish_ms']) for t in times):
            raise ValueError('invalid entry data-readiness clock')
        total = sum(int(e['decoded_bytes']) for e in cells)
        ready = 0
        halfway = None
        for e in cells:
            ready += int(e['decoded_bytes'])
            if halfway is None and ready * 2 >= total:
                halfway = float(e['data_ready_ms'])
        readiness.append({'label': row['label'], 'first_data_ready_ms': min(times) if times else float(row['arrival_ms']),
                          'half_data_ready_ms': halfway if times else float(row['arrival_ms']),
                          'last_data_ready_ms': max(times) if times else float(row['arrival_ms'])})
    results.append({'delivery': delivery, 'mode': mode, 'cache_bytes': cache, 'cache_scope': a.cache_scope, 'rtt_ms': rtt,
                    'bytes_per_second': 12500000, 'concurrency': concurrency,
                    'requests': sum(int(r['requests']) for r in rows),
                    'dependency_rounds': sum(int(r['dependency_rounds']) for r in rows),
                    'transferred_bytes': sum(int(r['bytes']) for r in rows),
                    'network_busy_ms': sum(float(r['finish_ms']) - float(r['start_ms']) for r in rows),
                    'finish_ms': float(rows[-1]['finish_ms']), 'views': len(rows), 'data_readiness': readiness})
(a.output / 'summary.json').write_text(json.dumps({'model': 'serial demand replay; parallel read_many_at waves; shared bandwidth; no decode-time prediction',
    'binary_sha256': hashlib.sha256(a.binary.read_bytes()).hexdigest(),
    'trace_sha256': hashlib.sha256(a.trace.read_bytes()).hexdigest(), 'pixel_domains': reference, 'entry_pixel_domains': entry_reference, 'results': results}, indent=2) + '\n')
print(f'{len(results)} transport scenarios: identical decoded domains and hashes')
