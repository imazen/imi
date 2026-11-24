#!/usr/bin/env python3
"""Publish a hash-verified static viewer graph to R2; keep every earlier bundle.

Caller supplies R2_ACCOUNT_ID, R2_ACCESS_KEY_ID, R2_SECRET_ACCESS_KEY.
HTML is revalidated, fingerprinted bundle paths are immutable. No bucket or
CORS settings are changed. The custom domain must already serve this bucket.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

p = argparse.ArgumentParser()
p.add_argument('package', type=Path)
p.add_argument('--bucket', default='codec-corpus')
p.add_argument('--prefix', default='imij/explorer')
p.add_argument('--public-base', default='https://codec-corpus.r2.imazen.org/')
a = p.parse_args()
if a.prefix and any(part in ('', '.', '..') for part in a.prefix.split('/')):
    raise ValueError('object prefix')
env = dict(os.environ, AWS_ACCESS_KEY_ID=os.environ['R2_ACCESS_KEY_ID'],
           AWS_SECRET_ACCESS_KEY=os.environ['R2_SECRET_ACCESS_KEY'])
env.pop('AWS_SESSION_TOKEN', None)
command = ['aws', '--endpoint-url', 'https://' + os.environ['R2_ACCOUNT_ID'] + '.r2.cloudflarestorage.com']
mimes = {'.js': 'text/javascript', '.json': 'application/json', '.wasm': 'application/wasm',
         '.html': 'text/html; charset=utf-8', '.css': 'text/css; charset=utf-8',
         '.imi': 'application/octet-stream', '.jpg': 'image/jpeg', '.png': 'image/png',
         '.avif': 'image/avif', '.jxl': 'image/jxl', '.webp': 'image/webp'}
assets = []
for line in (a.package / 'SHA256SUMS').read_text().splitlines():
    sha, name = line.split('  ', 1)
    path = a.package / name
    if Path(name).is_absolute() or '..' in Path(name).parts:
        raise ValueError('asset path')
    if hashlib.sha256(path.read_bytes()).hexdigest() != sha:
        raise ValueError('package changed: ' + name)
    assets.append((sha, name, path))
# Entry HTML goes last, after its complete graph is available.
assets.sort(key=lambda item: item[1].endswith('.html'))
records = []
for sha, name, path in assets:
    key = (a.prefix + '/' if a.prefix else '') + name
    cache = 'public,max-age=31536000,immutable' if name.startswith('bundles/') else 'no-cache'
    subprocess.run(command + ['s3', 'cp', str(path), 's3://' + a.bucket + '/' + key,
                             '--content-type', mimes[path.suffix], '--cache-control', cache,
                             '--metadata', 'sha256=' + sha, '--only-show-errors'], env=env, check=True)
    records.append({'key': key, 'sha256': sha, 'bytes': path.stat().st_size})
print(json.dumps({'url': a.public_base.rstrip('/') + '/' + (a.prefix + '/' if a.prefix else '') + 'gigapixel/index.html',
                  'bucket': a.bucket, 'assets': records}, indent=2))
