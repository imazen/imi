"""Publish the explorer's data to a bucket served at its own origin, as origin-relative content-addressed objects.

Layout (every key is immutable; the catalog uses origin-relative URLs so the bucket can move without a rewrite):
  d/<archive sha256>/archive.bin            an archive up to --split-over bytes, byte-identical to the original file
  d/<archive sha256>/p/000000.bin ...       a larger archive as fixed-size parts (the last short); concatenated, the parts
                                            are the original file
  d/<archive sha256>/parts.xxh3             XXH3-64 (seed 0, default secret: the IMI cSUM/pSUM digest) of every part,
                                            little-endian u64s; the catalog records the table's own XXH3-64
  d/<index sha256>/index.bin                the tile sidecar
  p/<preview sha256>/preview.jpg            gallery previews
Large archives are streamed from the source bucket, split, hashed (whole-file SHA-256 must equal the catalog's, XXH3-64
per part) and uploaded part by part, so local disk holds only a few parts at once. Small objects are copied server-side.
Re-running skips archives whose state file exists. Credentials come from R2_ACCOUNT_ID / R2_ACCESS_KEY_ID /
R2_SECRET_ACCESS_KEY and are never logged.

usage: publish-gigapixels.py CATALOG_IN.json OUT_DIR --xxh3sum BIN [--only SHA ...] [--bucket gigapixels]
"""
import argparse
import concurrent.futures as futures
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import time
from urllib.parse import urlparse

p = argparse.ArgumentParser()
p.add_argument('catalog', type=Path)
p.add_argument('out', type=Path)
p.add_argument('--xxh3sum', type=Path, required=True)
p.add_argument('--src-bucket', default='codec-corpus')
p.add_argument('--bucket', default='gigapixels')
p.add_argument('--part-size', type=int, default=32 << 20)
p.add_argument('--split-over', type=int, default=128 << 20)
p.add_argument('--only', action='append', help='restrict to these archive sha256 (testing)')
p.add_argument('--archives', type=int, default=3, help='archives processed concurrently')
p.add_argument('--uploads', type=int, default=12, help='concurrent part uploads')
p.add_argument('--scratch', type=Path, default=Path(os.path.expanduser('~/tmp')))
a = p.parse_args()
a.out.mkdir(parents=True, exist_ok=True)
(a.out / 'state').mkdir(exist_ok=True)
env = dict(os.environ, AWS_ACCESS_KEY_ID=os.environ['R2_ACCESS_KEY_ID'], AWS_SECRET_ACCESS_KEY=os.environ['R2_SECRET_ACCESS_KEY'])
env.pop('AWS_SESSION_TOKEN', None)
ENDPOINT = ['aws', '--endpoint-url', 'https://' + os.environ['R2_ACCOUNT_ID'] + '.r2.cloudflarestorage.com']
IMMUTABLE = 'public,max-age=31536000,immutable'
log_lock = threading.Lock()


def log(message):
    with log_lock:
        print(time.strftime('%H:%M:%S'), message, flush=True)


def aws(*args, **kwargs):
    return subprocess.run(ENDPOINT + list(args), env=env, check=True, capture_output=True, text=True, **kwargs)


def head(bucket, key):
    return json.loads(aws('s3api', 'head-object', '--bucket', bucket, '--key', key).stdout)


def exists(bucket, key, size=None):
    try:
        info = head(bucket, key)
    except subprocess.CalledProcessError as error:
        if '404' in error.stderr or 'Not Found' in error.stderr:
            return None
        raise
    return info if size is None or info['ContentLength'] == size else None


def copy(src_key, dst_key, sha, content_type, size):
    if (info := exists(a.bucket, dst_key, size)) and info.get('Metadata', {}).get('sha256') == sha:
        return info['ETag']
    aws('s3', 'cp', f's3://{a.src_bucket}/{src_key}', f's3://{a.bucket}/{dst_key}', '--only-show-errors', '--metadata-directive', 'REPLACE',
        '--content-type', content_type, '--cache-control', IMMUTABLE, '--metadata', f'sha256={sha}')
    info = head(a.bucket, dst_key)
    if info['ContentLength'] != size or info['Metadata'].get('sha256') != sha:
        raise ValueError(f'copied object identity {dst_key}')
    return info['ETag']


def upload_file(path, dst_key, metadata, content_type='application/octet-stream'):
    aws('s3', 'cp', str(path), f's3://{a.bucket}/{dst_key}', '--only-show-errors', '--content-type', content_type,
        '--cache-control', IMMUTABLE, '--metadata', metadata)


def xxh3(path):
    out = subprocess.run([str(a.xxh3sum), str(path)], check=True, capture_output=True, text=True).stdout.split()
    return out[0], int(out[1])


def split_archive(sha, size, src_key):
    state_path = a.out / 'state' / f'{sha}.json'
    if state_path.exists():
        return json.loads(state_path.read_text())
    count = -(-size // a.part_size)
    started = time.monotonic()
    scratch = Path(tempfile.mkdtemp(prefix=f'gigapixels-{sha[:12]}-', dir=a.scratch))
    digests, sem = [None] * count, threading.BoundedSemaphore(a.uploads + 2)
    whole = hashlib.sha256()

    def ship(index, path):
        try:
            digest, length = xxh3(path)
            expected = a.part_size if index < count - 1 else size - a.part_size * (count - 1)
            if length != expected:
                raise ValueError(f'part {index} length')
            key = f'd/{sha}/p/{index:06d}.bin'
            if not (info := exists(a.bucket, key, length)) or info.get('Metadata', {}).get('xxh3') != digest:
                upload_file(path, key, f'xxh3={digest}')
            digests[index] = digest
        finally:
            path.unlink(missing_ok=True)
            sem.release()

    with futures.ThreadPoolExecutor(a.uploads) as pool:
        jobs = []
        source = subprocess.Popen(ENDPOINT + ['s3', 'cp', f's3://{a.src_bucket}/{src_key}', '-'], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        total = 0
        for index in range(count):
            want = min(a.part_size, size - total)
            chunk = source.stdout.read(want)
            while len(chunk) < want:
                more = source.stdout.read(want - len(chunk))
                if not more:
                    raise ValueError(f'{sha}: source ended after {total + len(chunk)} of {size} bytes')
                chunk += more
            total += len(chunk)
            whole.update(chunk)
            path = scratch / f'{index:06d}.bin'
            path.write_bytes(chunk)
            sem.acquire()
            jobs.append(pool.submit(ship, index, path))
            if index % 50 == 49:
                log(f'{sha[:12]} parts {index + 1}/{count}')
        if source.stdout.read(1):
            raise ValueError(f'{sha}: source longer than {size} bytes')
        source.stdout.close()
        if source.wait() != 0:
            raise ValueError(f'{sha}: source read failed: {source.stderr.read().decode()[:300]}')
        for job in jobs:
            job.result()
    scratch.rmdir()
    if whole.hexdigest() != sha:
        raise ValueError(f'{sha}: source bytes hash to {whole.hexdigest()}')
    table = b''.join(int(d, 16).to_bytes(8, 'little') for d in digests)
    table_path = a.out / 'tables' / f'{sha}.xxh3'
    table_path.parent.mkdir(exist_ok=True)
    table_path.write_bytes(table)
    table_xxh3, _ = xxh3(table_path)
    upload_file(table_path, f'd/{sha}/parts.xxh3', f'xxh3={table_xxh3}')
    state = {'count': count, 'size': a.part_size, 'tableXxh3': table_xxh3, 'tableBytes': len(table), 'digests': digests,
             'seconds': round(time.monotonic() - started, 1)}
    state_path.write_text(json.dumps(state))
    log(f'{sha[:12]} split into {count} parts of {a.part_size} bytes, sha256 matches the catalog ({state["seconds"]} s)')
    return state


catalog = json.loads(a.catalog.read_text())
jobs = {}  # archive sha -> (src_key, size)
indexes, previews = {}, {}
for dataset in catalog['datasets']:
    if dataset.get('thumbnail'):
        t = dataset['thumbnail']
        previews[t['sha256']] = (urlparse(t['url']).path.lstrip('/'), t['bytes'])
    for qualities in dataset['representations'].values():
        for rep in qualities.values():
            jobs[rep['sha256']] = (urlparse(rep['url']).path.lstrip('/'), rep['encodedBytes'])
            indexes[rep['index']['sha256']] = (urlparse(rep['index']['url']).path.lstrip('/'), rep['index']['bytes'])
if a.only:
    jobs = {k: v for k, v in jobs.items() if k in a.only}
log(f'{len(jobs)} archives, {len(indexes)} indexes, {len(previews)} previews; split over {a.split_over} into {a.part_size}-byte parts')
results, etags = {}, {}


def handle(item):
    sha, (key, size) = item
    if size > a.split_over:
        results[sha] = split_archive(sha, size, key)
    else:
        etags[sha] = copy(key, f'd/{sha}/archive.bin', sha, 'application/octet-stream', size)
    return sha


with futures.ThreadPoolExecutor(a.archives) as pool:
    for done, sha in enumerate(pool.map(handle, sorted(jobs.items(), key=lambda kv: -kv[1][1])), 1):
        if done % 20 == 0 or done == len(jobs):
            log(f'archives {done}/{len(jobs)}')
if not a.only:
    with futures.ThreadPoolExecutor(8) as pool:
        list(pool.map(lambda kv: copy(kv[1][0], f'd/{kv[0]}/index.bin', kv[0], 'application/octet-stream', kv[1][1]), indexes.items()))
        list(pool.map(lambda kv: copy(kv[1][0], f'p/{kv[0]}/preview.jpg', kv[0], 'image/jpeg', kv[1][1]), previews.items()))
    log('indexes and previews copied')
    # Rewrite the catalog: origin-relative URLs, parts for large archives, the destination's own ETags.
    for dataset in catalog['datasets']:
        if dataset.get('thumbnail'):
            dataset['thumbnail']['url'] = f"/p/{dataset['thumbnail']['sha256']}/preview.jpg"
        for qualities in dataset['representations'].values():
            for rep in qualities.values():
                sha = rep['sha256']
                rep['index']['url'] = f"/d/{rep['index']['sha256']}/index.bin"
                if sha in results:
                    state = results[sha]
                    rep.pop('url', None)
                    rep.pop('etag', None)
                    rep['multipart'] = False
                    rep['parts'] = {'size': state['size'], 'count': state['count'], 'base': f'/d/{sha}/p',
                                    'table': {'url': f'/d/{sha}/parts.xxh3', 'bytes': state['tableBytes'], 'xxh3': state['tableXxh3']}}
                else:
                    rep['url'] = f'/d/{sha}/archive.bin'
                    rep['etag'] = etags[sha]
    # The viewer reads `datasets` only; the original single-file Earth fields at the top level are dropped.
    catalog = {'datasets': catalog['datasets']}
    out = a.out / 'catalog-gigapixels.json'
    out.write_text(json.dumps(catalog, indent=1, ensure_ascii=False) + '\n')
    log(f'wrote {out}')
