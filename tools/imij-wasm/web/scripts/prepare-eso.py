"""Import the original VISTA PSB through its ICC, then prepare a complete IMI pyramid.

Use the standard resource wrapper. Source download is a separate verified step.
Output is exclusive; original source and prior catalog are never modified.
"""
from pathlib import Path
import argparse
import hashlib
import json
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('importer', type=Path)
parser.add_argument('pyramid', type=Path)
parser.add_argument('source', type=Path)
parser.add_argument('prior_catalog', type=Path)
parser.add_argument('output', type=Path)
a = parser.parse_args()
for key in ('importer', 'pyramid', 'source', 'prior_catalog'):
    setattr(a, key, getattr(a, key).resolve(strict=True))
a.output.mkdir(exist_ok=False)


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def run(command):
    print(json.dumps([str(v) for v in command]), flush=True)
    subprocess.run([str(v) for v in command], check=True)


if a.source.stat().st_size != 26455654863:
    raise ValueError('original PSB extent')
source_hash = digest(a.source)
raw, icc = a.output / 'source.rgb', a.output / 'source.icc'
run([a.importer, a.source, raw, icc])
width, height = 108199, 81503
if raw.stat().st_size != width * height * 3:
    raise ValueError('converted original raster extent')
provenance = {'source_url': 'https://cdn2.eso.org/images/original/eso1242a.psb',
              'source_sha256': source_hash, 'raw_sha256': digest(raw),
              'icc_sha256': digest(icc), 'importer_sha256': digest(a.importer),
              'pyramid_sha256': digest(a.pyramid), 'width': width, 'height': height,
              'conversion': 'embedded LStar-RGB-v4 ICC -> sRGB RGB8, zenpixels-convert MoxCms relative colorimetric; no upscaling'}
(a.output / 'source-provenance.json').write_text(json.dumps(provenance, indent=2) + '\n')
run([a.pyramid, 'complete-raw', width, height, raw, a.output / 'milkyway'])
catalog = json.loads(a.prior_catalog.read_text())
for scene in catalog['datasets']:
    if scene['id'] == 'earth':
        scene['description'] = 'Four adjacent original 500 m NASA tiles, July 2004. No upscaling; full image and half-step pyramid, lossless and lossy RGB8.'
        catalog['description'] = scene['description']
if any(s['id'] == 'milkyway' for s in catalog['datasets']):
    raise ValueError('scene already present')
catalog['datasets'].append({
    'id': 'milkyway', 'title': 'VISTA Milky Way · 8.82 gigapixels',
    'width': width, 'height': height, 'pixels': width * height,
    'groupWidth': 512, 'groupHeight': 256, 'maxSourcePixels': 16777216, 'maxZoomOut': 256,
    'initial': {'x': width / 2, 'y': height / 2, 'scale': 1},
    'credit': 'ESO/VVV Survey/D. Minniti; acknowledgement Ignacio Toledo, Martin Kornmesser',
    'sourceUrl': 'https://www.eso.org/public/images/eso1242a/',
    'description': 'Original 108,199 × 81,503 infrared color rendering. Embedded LStar RGB ICC converted to sRGB; no upscaling. Lossless means exact converted RGB8.',
    'sources': [provenance], 'representations': {'lossless': {}, 'lossy': {}}})
(a.output / 'catalog-input.json').write_text(json.dumps(catalog, indent=2) + '\n')
print('Original Milky Way, full lossless/lossy and complete half-step pyramid prepared', flush=True)
