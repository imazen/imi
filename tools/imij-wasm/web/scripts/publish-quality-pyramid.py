"""Publish an additional lossy quality for every level of the scenes it covers.

Reads `prepare-quality-pyramid.py` output, verifies the encoder provenance and that each
cell's source rasters are the ones recorded for the scene's existing representations, uploads
archive + tile index to immutable content-hash keys (skipping objects already present with
the same hash and size) and writes a new catalog. The prior catalog is never modified.
usage: publish-quality-pyramid.py PREPARED PRIOR_CATALOG OUTPUT_CATALOG --quality-key lossy90 --target 90 --source-commit SHA
Credentials come from R2_ACCOUNT_ID / R2_ACCESS_KEY_ID / R2_SECRET_ACCESS_KEY (never logged).
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess

p = argparse.ArgumentParser()
p.add_argument('prepared', type=Path)
p.add_argument('prior', type=Path)
p.add_argument('output', type=Path)
p.add_argument('--quality-key', required=True)
p.add_argument('--target', type=float, help='JpegQuality target; omit for lossless')
p.add_argument('--source-commit', required=True)
p.add_argument('--scene', action='append', help='restrict to these scene ids (default: every scene in the catalog)')
p.add_argument('--reference', type=Path, help='preparation.json of the existing representations (source raster identities)')
a = p.parse_args()
if a.output.exists():
    raise FileExistsError(a.output)
catalog = json.loads(a.prior.read_text())
endpoint = 'https://' + os.environ['R2_ACCOUNT_ID'] + '.r2.cloudflarestorage.com'
os.environ['AWS_ACCESS_KEY_ID'] = os.environ['R2_ACCESS_KEY_ID']
os.environ['AWS_SECRET_ACCESS_KEY'] = os.environ['R2_SECRET_ACCESS_KEY']
os.environ.pop('AWS_SESSION_TOKEN', None)
records = []


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def publish(path):
    sha, size = digest(path), path.stat().st_size
    key = 'imij/nasa/' + sha + '/' + path.name
    head_cmd = ['aws', '--endpoint-url', endpoint, 's3api', 'head-object', '--bucket', 'codec-corpus', '--key', key]
    head = subprocess.run(head_cmd, capture_output=True, text=True)
    if head.returncode:
        if '(404)' not in head.stderr and 'Not Found' not in head.stderr:
            raise RuntimeError(head.stderr)
        subprocess.run(['aws', '--endpoint-url', endpoint, 's3', 'cp', str(path), 's3://codec-corpus/' + key, '--no-progress',
                        '--only-show-errors', '--content-type', 'application/octet-stream',
                        '--cache-control', 'public,max-age=31536000,immutable', '--metadata', 'sha256=' + sha], check=True)
        head = subprocess.run(head_cmd, capture_output=True, text=True, check=True)
    info = json.loads(head.stdout)
    if info['ContentLength'] != size or info['Metadata']['sha256'] != sha:
        raise ValueError('published object identity ' + key)
    value = {'url': 'https://codec-corpus.r2.imazen.org/' + key, 'sha256': sha, 'encodedBytes': size,
             'etag': info['ETag'], 'multipart': False}
    records.append({'file': str(path.relative_to(a.prepared)), **value})
    print(path.relative_to(a.prepared), size, sha, flush=True)
    return value


complete = json.loads((a.prepared / 'complete.json').read_text())
prepared = json.loads((a.prepared / 'preparation.json').read_text())
if complete['count'] != len(prepared):
    raise ValueError('incomplete preparation')
reference = {}
if a.reference:
    for row in json.loads(a.reference.read_text()):
        reference.setdefault((row['scene'], row['level']), [(v['bytes'], v['sha256']) for v in row['inputs']])
expected = {(s['id'], level['key']) for s in catalog['datasets'] for level in s['levels'] if not a.scene or s['id'] in a.scene}
actual = {(r['scene'], r['level']) for r in prepared if r['quality'] == a.quality_key and (not a.scene or r['scene'] in a.scene)}
if expected != actual or len([r for r in prepared if r['quality'] == a.quality_key and (not a.scene or r['scene'] in a.scene)]) != len(actual):
    raise ValueError(f'domain: missing {sorted(expected - actual)[:5]} extra {sorted(actual - expected)[:5]}')
for item in prepared:
    if item['quality'] != a.quality_key or (a.scene and item['scene'] not in a.scene):
        continue
    if item['effort'] != 10 or item['rc'] != 0 or item['binarySha256'] != complete['binarySha256'] or item['target'] != a.target:
        raise ValueError('encoder provenance')
    if reference and [(v['bytes'], v['sha256']) for v in item['inputs']] != reference[(item['scene'], item['level'])]:
        raise ValueError('source raster identity')
    scene = next(s for s in catalog['datasets'] if s['id'] == item['scene'])
    old = scene['representations'].get('lossless', {}).get(item['level'])
    if old and (old['width'], old['height']) != (item['width'], item['height']):
        raise ValueError('geometry')
    path = a.prepared / item['file']
    if digest(path) != item['sha256'] or digest(path.with_suffix('.bin')) != item['indexSha256']:
        raise ValueError('prepared identity')
    archive, index = publish(path), publish(path.with_suffix('.bin'))
    archive.update(kind='tiles', width=item['width'], height=item['height'], divisor=item['divisor'], effort=10,
                   index={'url': index['url'], 'sha256': index['sha256'], 'bytes': index['encodedBytes']},
                   quality='IMI lossless effort10; ordinary 512x256 cells' if a.target is None else f"IMI JpegQuality target{a.target:g}, Fast decode, effort10; ordinary 512x256 cells",
                   encoder={'binarySha256': item['binarySha256'], 'sourceCommit': a.source_commit})
    if a.target is not None:
        archive['qualityTarget'] = a.target
    elif item.get('rois'):
        archive['rois'] = item['rois']
    scene['representations'].setdefault(a.quality_key, {})[item['level']] = archive
a.output.write_text(json.dumps(catalog, indent=2) + '\n')
a.output.with_name(a.output.stem + '-objects.json').write_text(json.dumps(records, indent=2) + '\n')
