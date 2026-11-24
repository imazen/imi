"""Read back a random sample of published parts and compare their XXH3-64 with the digests recorded at publication,
and check each sampled archive's part count, sizes and parts.xxh3 table in the bucket.
usage: verify-gigapixels.py OUT_DIR --xxh3sum BIN [--sample 150] [--bucket gigapixels]
"""
import argparse
import json
import os
from pathlib import Path
import random
import subprocess
import tempfile

p = argparse.ArgumentParser()
p.add_argument('out', type=Path)
p.add_argument('--xxh3sum', type=Path, required=True)
p.add_argument('--sample', type=int, default=150)
p.add_argument('--bucket', default='gigapixels')
a = p.parse_args()
env = dict(os.environ, AWS_ACCESS_KEY_ID=os.environ['R2_ACCESS_KEY_ID'], AWS_SECRET_ACCESS_KEY=os.environ['R2_SECRET_ACCESS_KEY'])
env.pop('AWS_SESSION_TOKEN', None)
endpoint = ['aws', '--endpoint-url', 'https://' + os.environ['R2_ACCOUNT_ID'] + '.r2.cloudflarestorage.com']
states = {f.stem: json.loads(f.read_text()) for f in (a.out / 'state').glob('*.json')}
rng = random.Random(20261017)
picks = [(sha, rng.randrange(s['count'])) for sha, s in states.items()]
extra = [(sha, i) for sha, s in states.items() for i in (0, s['count'] - 1)]
sample = sorted(set(picks + extra))
rng.shuffle(sample)
sample = sample[:a.sample]
bad = checked = nbytes = 0
for sha, index in sample:
    state = states[sha]
    key = f'd/{sha}/p/{index:06d}.bin'
    with tempfile.NamedTemporaryFile(dir=os.path.expanduser('~/tmp'), suffix='.part') as handle:
        subprocess.run(endpoint + ['s3', 'cp', f's3://{a.bucket}/{key}', handle.name, '--only-show-errors'], env=env, check=True)
        digest, length = subprocess.run([str(a.xxh3sum), handle.name], check=True, capture_output=True, text=True).stdout.split()[:2]
    expected = state['size'] if index < state['count'] - 1 else None
    checked += 1
    nbytes += int(length)
    if digest != state['digests'][index] or (expected and int(length) != expected):
        bad += 1
        print('MISMATCH', key, digest, state['digests'][index], length)
tables = 0
for sha, state in states.items():
    listing = subprocess.run(endpoint + ['s3', 'ls', f's3://{a.bucket}/d/{sha}/', '--recursive'], env=env, check=True, capture_output=True, text=True).stdout.split('\n')
    sizes = {line.split()[3]: int(line.split()[2]) for line in listing if line.strip()}
    parts = [k for k in sizes if '/p/' in k]
    if len(parts) != state['count'] or sizes.get(f'd/{sha}/parts.xxh3') != state['tableBytes']:
        bad += 1
        print('LISTING MISMATCH', sha, len(parts), state['count'])
    tables += 1
print(json.dumps({'archives_listed': tables, 'parts_read_back': checked, 'bytes_read_back': nbytes, 'failures': bad}))
