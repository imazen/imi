"""Encode additional lossy qualities for existing scene levels at IMI effort10.

Reuses the original RGB8 level rasters recorded by an earlier preparation
(`preparation.json` rows of any lossy quality) so no source pixel is regenerated.
Resumable: a finished cell (result.json rc 0, output hash still matching) is kept;
a half-written cell is moved aside to `<cell>.incomplete.<n>`, never deleted.
usage: prepare-quality-pyramid.py BINARY REFERENCE_PREPARATION.json OUTPUT --quality lossy90=90 [--quality lossless=lossless]
REFERENCE is a list of rows {scene, level, divisor, width, height, inputs:[{path,bytes,sha256}]}.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time

p = argparse.ArgumentParser()
p.add_argument('binary', type=Path)
p.add_argument('reference', type=Path, help='preparation.json whose inputs are the original level rasters')
p.add_argument('output', type=Path)
p.add_argument('--quality', action='append', required=True, metavar='KEY=TARGET')
p.add_argument('--scene', action='append', help='restrict to scene ids')
p.add_argument('--max-level-pixels', type=int, help='skip levels with more pixels (staged runs)')
p.add_argument('--min-level-pixels', type=int, default=0)
a = p.parse_args()
a.binary = a.binary.resolve()
# KEY=TARGET for lossy JpegQuality, or KEY=lossless.
qualities = [(k, None if v == 'lossless' else float(v)) for k, v in (item.split('=', 1) for item in a.quality)]
a.output.mkdir(exist_ok=True)


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def rois(path, w, h):
    # Three 32x32 decoded-sample references from the original level raster, for browser checks.
    rw, rh = min(w, 32), min(h, 32)
    if path.stat().st_size != w * h * 3:
        raise ValueError('raw raster extent')
    result = []
    with path.open('rb') as stream:
        for x, y in [(min(w // 3, w - rw), min(h // 3, h - rh)), (min(w // 2, w - rw), min(h // 2, h - rh)), (w - rw, h - rh)]:
            pixels = bytearray()
            for yy in range(y, y + rh):
                stream.seek((yy * w + x) * 3)
                pixels.extend(stream.read(rw * 3))
            result.append({'x': x, 'y': y, 'width': rw, 'height': rh, 'sha256': hashlib.sha256(pixels).hexdigest()})
    return result


binary_sha = digest(a.binary)
reference = json.loads(a.reference.read_text())
cells = {}
for row in reference:
    cells.setdefault((row['scene'], row['level']), row)
records_path = a.output / 'preparation.json'
records = json.loads(records_path.read_text()) if records_path.exists() else []
done = {(r['scene'], r['level'], r['quality']): r for r in records}
if any(r['binarySha256'] != binary_sha for r in records):
    raise ValueError('output already holds cells from another encoder binary')
order = sorted(cells.values(), key=lambda r: r['width'] * r['height'])
for row in order:
    pixels = row['width'] * row['height']
    if a.scene and row['scene'] not in a.scene or pixels > (a.max_level_pixels or pixels) or pixels < a.min_level_pixels:
        continue
    paths = [Path(item['path']) for item in row['inputs']]
    before = [{'path': str(path), 'bytes': path.stat().st_size, 'sha256': digest(path)} for path in paths]
    if [(v['bytes'], v['sha256']) for v in before] != [(v['bytes'], v['sha256']) for v in row['inputs']]:
        raise ValueError(f"{row['scene']} {row['level']}: source raster differs from the reference preparation")
    for key, target in qualities:
        cell = a.output / row['scene'] / row['level'] / key
        output = cell / 'image.imijset'
        if (row['scene'], row['level'], key) in done:
            record = done[(row['scene'], row['level'], key)]
            if output.exists() and digest(output) == record['sha256']:
                continue
        if cell.exists():
            n = 0
            while cell.with_name(f'{key}.incomplete.{n}').exists():
                n += 1
            cell.rename(cell.with_name(f'{key}.incomplete.{n}'))
        cell.mkdir(parents=True)
        width, height = row['width'], row['height']
        side = [str(int(round((width // 2))))] if len(paths) == 4 else [str(width), str(height)]
        kind = 'lossless' if target is None else 'lossy'
        command = [str(a.binary), kind + ('-mosaic' if len(paths) == 4 else '-raw'), *side, *map(str, paths), str(output)]
        if target is not None:
            command.append(str(int(target) if target == int(target) else target))
        record = {'scene': row['scene'], 'level': row['level'], 'divisor': row['divisor'], 'quality': key, 'target': target,
                  'effort': 10, 'width': width, 'height': height, 'inputs': before, 'binarySha256': binary_sha, 'command': command}
        if target is None and len(paths) == 1:
            record['rois'] = rois(paths[0], width, height)
        (cell / 'command.json').write_text(json.dumps(record, indent=2) + '\n')
        started = time.monotonic()
        with (cell / 'encode.log').open('xb') as log:
            result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT)
        record.update(rc=result.returncode, seconds=time.monotonic() - started)
        (cell / 'result.json').write_text(json.dumps(record, indent=2) + '\n')
        result.check_returncode()
        for path, item in zip(paths, before):
            if path.stat().st_size != item['bytes'] or digest(path) != item['sha256']:
                raise ValueError('input changed during encode')
        record.update(file=str(output.relative_to(a.output)), sha256=digest(output), encodedBytes=output.stat().st_size,
                      indexSha256=digest(output.with_suffix('.bin')))
        records = [r for r in records if (r['scene'], r['level'], r['quality']) != (row['scene'], row['level'], key)] + [record]
        records_path.write_text(json.dumps(records, indent=2) + '\n')
        print(row['scene'], row['level'], key, record['encodedBytes'], f"{record['seconds']:.1f}s", flush=True)
if digest(a.binary) != binary_sha:
    raise ValueError('encoder changed')
expected = {(r['scene'], r['level'], k) for r in order for k, _ in qualities if (not a.scene or r['scene'] in a.scene)
            and r['width'] * r['height'] <= (a.max_level_pixels or r['width'] * r['height']) and r['width'] * r['height'] >= a.min_level_pixels}
if expected <= {(r['scene'], r['level'], r['quality']) for r in records}:
    (a.output / 'complete.json').write_text(json.dumps({'count': len(records), 'binarySha256': binary_sha}) + '\n')
