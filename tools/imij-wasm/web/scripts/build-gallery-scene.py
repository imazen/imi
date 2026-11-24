"""Describe a newly imported gallery scene: encoder input spec, then catalog entry.

spec:  build-gallery-scene.py spec ID IMPORT.json RASTER.rgb PYRAMID_DIR OUT_SPEC.json
       Writes the rows prepare-quality-pyramid.py takes (one per level: raster path, bytes, SHA-256).
entry: build-gallery-scene.py entry ID SOURCES.json IMPORT.json SPEC.json OUT_ENTRY.json
           --title T --description D --collection C --tag X ... [--thumbnail THUMB.json --thumbnail-url U --thumbnail-source S]
       Writes a catalog dataset with levels and empty representations; publish-quality-pyramid.py fills them.
"""
import argparse
import hashlib
import json
from pathlib import Path

LEVEL_KEYS = {1: 'full', 2: 'half', 4: 'quarter'}


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def key(divisor):
    return LEVEL_KEYS.get(divisor, f'level-{divisor}')


p = argparse.ArgumentParser()
sub = p.add_subparsers(dest='cmd', required=True)
s = sub.add_parser('spec')
s.add_argument('id')
s.add_argument('imported', type=Path)
s.add_argument('raster', type=Path)
s.add_argument('pyramid', type=Path)
s.add_argument('out', type=Path)
e = sub.add_parser('entry')
e.add_argument('id')
e.add_argument('sources', type=Path)
e.add_argument('imported', type=Path)
e.add_argument('spec', type=Path)
e.add_argument('out', type=Path)
e.add_argument('--title', required=True)
e.add_argument('--description', required=True)
e.add_argument('--collection', required=True)
e.add_argument('--tag', action='append', default=[])
e.add_argument('--credit')
e.add_argument('--thumbnail', type=Path)
e.add_argument('--thumbnail-url')
e.add_argument('--thumbnail-source')
e.add_argument('--thumbnail-alt')
e.add_argument('--conversion', default=None)
a = p.parse_args()
imported = json.loads(a.imported.read_text())
w, h = imported['width'], imported['height']
if a.cmd == 'spec':
    levels = json.loads((a.pyramid / 'levels.json').read_text())
    rows = [{'scene': a.id, 'level': 'full', 'divisor': 1, 'width': w, 'height': h, 'path': a.raster}]
    for item in levels:
        d = item['divisor']
        if (item['width'], item['height']) != ((w + d - 1) // d, (h + d - 1) // d):
            raise ValueError('pyramid geometry')
        rows.append({'scene': a.id, 'level': key(d), 'divisor': d, 'width': item['width'], 'height': item['height'],
                     'path': a.pyramid / f'level-{d}' / 'source.rgb'})
    spec = []
    for row in rows:
        path = row.pop('path')
        if path.stat().st_size != row['width'] * row['height'] * 3:
            raise ValueError(f'{path}: raster extent')
        row['quality'] = 'level'
        row['inputs'] = [{'path': str(path), 'bytes': path.stat().st_size, 'sha256': digest(path)}]
        spec.append(row)
    if spec[0]['inputs'][0]['sha256'] != imported['raw_sha256']:
        raise ValueError('level-0 raster differs from the import record')
    a.out.write_text(json.dumps(spec, indent=1) + '\n')
else:
    sources = {x['id']: x for x in json.loads(a.sources.read_text())['sources']}
    src = sources[a.id]
    spec = json.loads(a.spec.read_text())
    commons = src.get('commons', {})
    license_name = commons.get('license') or src.get('licenseName') or src['claimed']['license']
    entry = {
        'id': a.id, 'title': a.title, 'width': w, 'height': h, 'pixels': w * h,
        'groupWidth': 512, 'groupHeight': 256, 'maxSourcePixels': 16777216, 'maxZoomOut': spec[-1]['divisor'],
        'initial': {'x': w / 2, 'y': h / 2, 'scale': 1},
        'credit': a.credit or commons.get('artist') or src['claimed']['credit'],
        'sourceUrl': src['sourceUrl'], 'description': a.description, 'collection': a.collection, 'tags': a.tag,
        'license': {'name': license_name, 'url': commons.get('licenseUrl') or src.get('licenseUrl'),
                    'claimed': src['claimed']['license'], 'verified': src['verified']},
        'sources': [{'source_url': q['url'], 'source_sha256': q['sha256'], 'raw_sha256': imported['raw_sha256'],
                     'width': q['width'], 'height': q['height'], 'part': q['part'], 'conversion': a.conversion or imported['color']}
                    for q in src['usedParts']] if 'usedParts' in src else
                   [{'source_url': src['downloadUrl'], 'source_sha256': src.get('downloadSha256') or imported.get('source_sha256'),
                     'raw_sha256': imported['raw_sha256'], 'width': w, 'height': h,
                     'conversion': a.conversion or imported['color']}],
        'levels': [{'key': row['level'], 'divisor': row['divisor']} for row in spec],
        'representations': {},
    }
    if a.thumbnail:
        t = json.loads(a.thumbnail.read_text())
        entry['thumbnail'] = {'id': a.id, 'sourceUrl': a.thumbnail_source or 'derived from the prepared level raster',
                              'alt': a.thumbnail_alt or a.title + ' preview', 'sha256': t['sha256'], 'bytes': t['bytes'],
                              'width': t['width'], 'height': t['height'], 'url': a.thumbnail_url}
    a.out.write_text(json.dumps(entry, indent=1, ensure_ascii=False) + '\n')
