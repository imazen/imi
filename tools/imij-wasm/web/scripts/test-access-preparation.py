"""Real split-object hydration/replay test using a retained browser capture; no WAN.

Caller supplies capture and SHA-verified archive inventory. Missing inputs fail.
All generated fault cases and command output remain in a fresh output directory.
"""
import argparse
import copy
import csv
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess

spec = importlib.util.spec_from_file_location('prepare', Path(__file__).with_name('prepare-access-trace.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def run(command, prefix):
    prefix.with_suffix('.command.json').write_text(json.dumps(command) + '\n')
    with prefix.with_suffix('.log').open('x') as log:
        result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=False)
    prefix.with_suffix('.rc').write_text(str(result.returncode) + '\n')
    if result.returncode:
        raise RuntimeError(f'{prefix}: failed')


p = argparse.ArgumentParser(description=__doc__)
p.add_argument('replay', type=Path)
p.add_argument('layout', type=Path)
p.add_argument('capture', type=Path)
p.add_argument('inputs', type=Path)
p.add_argument('output', type=Path)
a = p.parse_args()
a.output.mkdir(parents=True, exist_ok=False)
trace = json.loads(a.capture.read_text())
objects = {r['sha256']: r for r in json.loads((a.inputs / 'inventory.json').read_text())}
for sha, record in objects.items():
    path = Path(record['path'])
    if path.stat().st_size != record['bytes'] or digest(path) != sha:
        raise ValueError('retained source mismatch')
    parts = a.output / sha
    parts.mkdir()
    size = 16384
    with path.open('rb') as f:
        i = 0
        while chunk := f.read(size):
            (parts / f'{i:06}.bin').write_bytes(chunk)
            i += 1
    for scene in trace['catalog']['datasets']:
        for reps in scene['representations'].values():
            for rep in reps.values():
                if rep['sha256'] == sha:
                    rep['parts'] = {'size': size, 'count': i, 'base': parts.resolve().as_uri() + '/'}
                    rep['url'] = (parts / 'must-not-fetch-concatenated-file').resolve().as_uri()
(a.output / 'capture.json').write_text(json.dumps(trace) + '\n')
module.prepare(trace, a.output / 'hydrated', 16777216, a.layout)
reference = None
for name, path in [('original', a.inputs / 'all.tsv'), ('parts', a.output / 'hydrated/all.tsv')]:
    for mode in ['ranges', 'whole']:
        prefix = a.output / f'{name}-{mode}'
        run([str(a.replay.resolve()), str(path), str(prefix), mode, '65536', '80', '12500000', '8', 'batch', 'shared'], a.output / f'{name}-{mode}-run')
        with prefix.with_suffix('.views.tsv').open() as f:
            rows = list(csv.DictReader(f, delimiter='\t'))
        pixels = [(r['label'], r['decoded_bytes'], r['xxh3']) for r in rows]
        if reference is None:
            reference = pixels
        if pixels != reference:
            raise ValueError('hydrated split sources changed pixels')
        if name == 'parts':
            with prefix.with_suffix('.ranges.tsv').open() as f:
                events = list(csv.DictReader(f, delimiter='\t'))
            if not events or any(int(e['end_exclusive']) > int(e['object_length']) or int(e['object_length']) > 16384 for e in events):
                raise ValueError('physical request crossed its part')

# Bad whole identity and invalid part geometry must fail, rather than produce a trace.
rejections = []
for name in ['bad-whole-hash', 'bad-part-count', 'missing-layout-binary']:
    invalid = copy.deepcopy(trace)
    view = invalid['views'][0]['message']
    scene = next(s for s in invalid['catalog']['datasets'] if s['id'] == view['scene'])
    rep = scene['representations'][view['quality']][view['level']]
    if name == 'bad-whole-hash':
        rep['sha256'] = '0' * 64
    elif name == 'bad-part-count':
        rep['parts']['count'] += 1
    try:
        module.prepare(invalid, a.output / name, 16777216, None if name == 'missing-layout-binary' else a.layout)
    except ValueError as error:
        rejections.append({'case': name, 'error': str(error)})
    else:
        raise AssertionError(f'{name} was accepted')
(a.output / 'results.json').write_text(json.dumps({'pixel_domains': reference, 'rejected': rejections,
    'objects': len(objects), 'replay_sha256': digest(a.replay), 'layout_sha256': digest(a.layout)}, indent=2) + '\n')
print(f'{len(objects)} real split objects hydrated; four replays exact; three invalid inputs rejected')
