"""Prepare one new gallery scene from a downloaded source, stage by stage, resumable.

Stages (each skipped when its output exists; never overwrites): import the source to a raw RGB8
level-0 raster (gallery_import), resize every half-step level straight from it (nasa_pyramid
rasters-raw), describe the level rasters, make a preview thumbnail from a prepared level, then encode
lossless + lossy targets 50/70/80/90 at effort 10 (prepare-quality-pyramid.py).
usage: prepare-gallery-scene.py ID KIND(jpeg|png|jpeg-grid|tiff|psb|tiff-grid) OUT_DIR SOURCE [SOURCE...] --import-bin B --pyramid-bin B [--grid COLSxROWS]
(jpeg-grid and tiff-grid sources are the parts in row-major order; --grid COLSxROWS.)
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

p = argparse.ArgumentParser()
p.add_argument('id')
p.add_argument('kind', choices=['jpeg', 'png', 'jpeg-grid', 'tiff', 'psb', 'tiff-grid'])
p.add_argument('out', type=Path)
p.add_argument('source', type=Path, nargs='+')
p.add_argument('--grid')
p.add_argument('--psb-bin', type=Path, help='psb_srgb binary (kind psb)')
p.add_argument('--import-bin', type=Path, required=True)
p.add_argument('--pyramid-bin', type=Path, required=True)
p.add_argument('--stop-after', choices=['import', 'levels', 'spec', 'thumb', 'encode'], default='encode')
a = p.parse_args()
here = Path(__file__).resolve().parent
a.out.mkdir(parents=True, exist_ok=True)
raw, record = a.out / 'level0.rgb', a.out / 'import.json'


def run(command, log):
    print('+', ' '.join(map(str, command)), flush=True)
    with (a.out / log).open('xb') as handle:
        subprocess.run(command, stdout=handle, stderr=subprocess.STDOUT, check=True)


if not record.exists():
    if raw.exists():
        raw.rename(raw.with_name(f'level0.rgb.incomplete.{len(list(a.out.glob("level0.rgb.incomplete.*")))}'))
    if a.kind == 'jpeg-grid':
        cols, rows = a.grid.split('x')
        run([a.import_bin, 'jpeg-grid', cols, rows, *a.source, raw, record], 'import.log')
    elif a.kind == 'tiff-grid':
        cols, rows = (int(v) for v in a.grid.split('x'))
        parts = a.out / 'parts'
        parts.mkdir(exist_ok=True)
        size = None
        for i, source in enumerate(a.source):
            part, part_record = parts / f'part{i}.rgb', parts / f'part{i}.json'
            if not part_record.exists():
                if part.exists():
                    part.rename(part.with_name(f'part{i}.rgb.incomplete.{len(list(parts.glob(f"part{i}.rgb.incomplete.*")))}'))
                with (parts / f'part{i}.log').open('xb') as handle:
                    print('+ tiff', source, flush=True)
                    subprocess.run([a.import_bin, 'tiff', source, part, part_record], stdout=handle, stderr=subprocess.STDOUT, check=True)
            info = json.loads(part_record.read_text())
            if size not in (None, (info['width'], info['height'])):
                raise ValueError('grid parts differ in size')
            size = (info['width'], info['height'])
        run([a.import_bin, 'raw-grid', str(cols), str(rows), str(size[0]), str(size[1]), *[parts / f'part{i}.rgb' for i in range(len(a.source))], raw, record], 'import.log')
    elif a.kind == 'psb':
        # Uncompressed merged RGB8 PSB with an embedded ICC: psb_srgb converts it to sRGB rows.
        icc = a.out / 'embedded.icc'
        run([a.psb_bin, a.source[0], raw, icc], 'import.log')
        text = (a.out / 'import.log').read_text()
        dims = [line for line in text.splitlines() if 'embedded ICC -> sRGB' in line][-1].split(':')[0]
        width, height = (int(v) for v in dims.split('x'))

        def digest(path):
            with path.open('rb') as stream:
                return hashlib.file_digest(stream, 'sha256').hexdigest()
        record.write_text(json.dumps({
            'source_sha256': digest(a.source[0]), 'source_bytes': a.source[0].stat().st_size, 'width': width, 'height': height,
            'raw_sha256': digest(raw), 'raw_bytes': raw.stat().st_size, 'icc_bytes': icc.stat().st_size, 'icc_sha256': digest(icc),
            'color': 'embedded ICC converted to sRGB (MoxCms relative colorimetric) by psb_srgb'}, indent=1) + '\n')
    else:
        run([a.import_bin, a.kind, a.source[0], raw, record], 'import.log')
imported = json.loads(record.read_text())
w, h = imported['width'], imported['height']
if a.stop_after == 'import':
    sys.exit(0)
if not (a.out / 'pyramid' / 'levels.json').exists():
    pyramid = a.out / 'pyramid'
    if pyramid.exists():
        pyramid.rename(a.out / f'pyramid.incomplete.{len(list(a.out.glob("pyramid.incomplete.*")))}')
    run([a.pyramid_bin, 'rasters-raw', str(w), str(h), raw, pyramid], 'levels.log')
if a.stop_after == 'levels':
    sys.exit(0)
spec = a.out / 'spec.json'
if not spec.exists():
    run([sys.executable, '-I', here / 'build-gallery-scene.py', 'spec', a.id, record, raw, a.out / 'pyramid', spec], 'spec.log')
if a.stop_after == 'spec':
    sys.exit(0)
rows = json.loads(spec.read_text())
thumb_row = next((r for r in reversed(rows) if max(r['width'], r['height']) >= 1280), rows[0])
thumb, thumb_json = a.out / 'preview.jpg', a.out / 'preview.json'
if not thumb.exists():
    level = Path(thumb_row['inputs'][0]['path'])
    with thumb_json.open('xb') as handle:
        print('+ thumb', level, flush=True)
        subprocess.run([a.import_bin, 'thumb', level, str(thumb_row['width']), str(thumb_row['height']), '1280', thumb], stdout=handle, check=True)
if a.stop_after == 'thumb':
    sys.exit(0)
quality = ['--quality', 'lossless=lossless', '--quality', 'webmap=70', '--quality', 'lossy=80', '--quality', 'lossy50=50', '--quality', 'lossy90=90']
run([sys.executable, '-I', here / 'prepare-quality-pyramid.py', a.pyramid_bin, spec, a.out / 'prepared', *quality], 'encode.log')
