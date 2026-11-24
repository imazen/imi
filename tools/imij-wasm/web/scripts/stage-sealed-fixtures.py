#!/usr/bin/env python3
"""Mirror every sealed fixture byte locally, reusing only hash-matching caches."""
import argparse
import concurrent.futures
import hashlib
import json
from pathlib import Path
import shutil
import threading

p = argparse.ArgumentParser()
p.add_argument('source', type=Path)
p.add_argument('destination', type=Path)
p.add_argument('--cache', type=Path, action='append', default=[])
p.add_argument('--max-bytes', type=int, required=True)
a = p.parse_args()
if a.max_bytes <= 0 or shutil.disk_usage(a.destination.parent).free < a.max_bytes + (512 << 20):
    raise RuntimeError('positive byte budget plus 512 MiB disk headroom required')
a.destination.mkdir()
lock = threading.Lock()
reserved = 0
records = []

def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

def copy(task):
    global reserved
    source, destination, expected, reused = task
    size = source.stat().st_size
    with lock:
        reserved += size
        if reserved > a.max_bytes:
            raise RuntimeError('fixture mirror byte budget exceeded; partial output preserved')
    with source.open('rb') as src, destination.open('xb') as dst:
        shutil.copyfileobj(src, dst, 1 << 20)
    actual = digest(destination)
    if actual != expected:
        raise RuntimeError(f'fixture hash mismatch: {source} -> {destination}: {actual} != {expected}')
    row = {'file': str(destination.relative_to(a.destination)), 'bytes': size,
           'sha256': actual, 'source': str(source), 'reused': reused}
    with lock:
        records.append(row)
        with (a.destination / 'copy-records.jsonl').open('a') as log:
            log.write(json.dumps(row) + '\n')

for domain in ['base', 'extra', 'superblocks']:
    src = a.source / domain
    dst = a.destination / domain
    dst.mkdir()
    seal_bytes = (src / 'sha256.json').read_bytes()
    seal = json.loads(seal_bytes)
    caches = []
    for root in a.cache:
        index = root / domain / 'sha256.json'
        if index.is_file():
            caches.append((root / domain, json.loads(index.read_text())['files']))
    tasks = []
    for name, expected in seal['files'].items():
        if Path(name).name != name:
            raise RuntimeError('flat sealed fixture filename required')
        source = src / name
        reused = False
        for root, files in caches:
            if files.get(name) == expected and (root / name).is_file():
                source = root / name
                reused = True
                break
        tasks.append((source, dst / name, expected, reused))
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(copy, tasks))
    (dst / 'sha256.json').write_bytes(seal_bytes)
    print(domain, len(tasks), 'sealed files verified', flush=True)
shutil.copyfile(a.source / 'composition.json', a.destination / 'composition.json')
summary = {'source': str(a.source), 'files': len(records), 'bytes': reserved,
           'reused': sum(r['reused'] for r in records),
           'recordsSha256': digest(a.destination / 'copy-records.jsonl')}
(a.destination / 'verification.json').write_text(json.dumps(summary, indent=2))
print(json.dumps(summary), flush=True)
