"""Add prepared scenes to a catalog: upload each preview, merge catalog entries (levels, credit, license,
provenance) with empty representations. Representations are filled afterwards by publish-quality-pyramid.py.
usage: add-gallery-scenes.py SCENES_DIR SOURCES.json DISPLAY.json BASE_CATALOG OUT_CATALOG --scene ID ...
Credentials from R2_ACCOUNT_ID / R2_ACCESS_KEY_ID / R2_SECRET_ACCESS_KEY.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

p = argparse.ArgumentParser()
p.add_argument('scenes', type=Path)
p.add_argument('sources', type=Path)
p.add_argument('display', type=Path)
p.add_argument('base', type=Path)
p.add_argument('out', type=Path)
p.add_argument('--scene', action='append', required=True)
a = p.parse_args()
if a.out.exists():
    raise FileExistsError(a.out)
here = Path(__file__).resolve().parent
catalog = json.loads(a.base.read_text())
display = {s['id']: s for s in json.loads(a.display.read_text())['scenes']}
endpoint = 'https://' + os.environ['R2_ACCOUNT_ID'] + '.r2.cloudflarestorage.com'
os.environ['AWS_ACCESS_KEY_ID'] = os.environ['R2_ACCESS_KEY_ID']
os.environ['AWS_SECRET_ACCESS_KEY'] = os.environ['R2_SECRET_ACCESS_KEY']
os.environ.pop('AWS_SESSION_TOKEN', None)
existing = {d['id'] for d in catalog['datasets']}
for sid in a.scene:
    if sid in existing:
        raise ValueError(f'{sid} already in the catalog')
    d, directory = display[sid], a.scenes / sid
    thumb = directory / 'preview.jpg'
    sha = hashlib.sha256(thumb.read_bytes()).hexdigest()
    key = f'imij/gallery/{sha}/preview.jpg'
    subprocess.run(['aws', '--endpoint-url', endpoint, 's3', 'cp', str(thumb), f's3://codec-corpus/{key}', '--no-progress',
                    '--only-show-errors', '--content-type', 'image/jpeg', '--cache-control', 'public,max-age=31536000,immutable',
                    '--metadata', f'sha256={sha}'], check=True)
    out = directory / 'catalog-entry.json'
    if out.exists():
        out.rename(out.with_name(f'catalog-entry.superseded-{len(list(directory.glob("catalog-entry*")))}.json'))
    command = [sys.executable, '-I', str(here / 'build-gallery-scene.py'), 'entry', sid, str(a.sources), str(directory / 'import.json'),
               str(directory / 'spec.json'), str(out), '--title', d['title'], '--description', d['description'],
               '--collection', d['collection'], '--thumbnail', str(directory / 'preview.json'),
               '--thumbnail-url', 'https://codec-corpus.r2.imazen.org/' + key, '--thumbnail-alt', d['thumbnailAlt'],
               '--thumbnail-source', 'derived from the prepared level raster (zenresize + zenjpeg q85)']
    for tag in d['tags']:
        command += ['--tag', tag]
    if d.get('credit'):
        command += ['--credit', d['credit']]
    subprocess.run(command, check=True)
    entry = json.loads(out.read_text())
    ratio = hashlib.sha256(thumb.read_bytes()).hexdigest()
    assert ratio == entry['thumbnail']['sha256'], 'preview changed'
    catalog['datasets'].append(entry)
    print(sid, key, flush=True)
a.out.write_text(json.dumps(catalog, indent=2, ensure_ascii=False) + '\n')
