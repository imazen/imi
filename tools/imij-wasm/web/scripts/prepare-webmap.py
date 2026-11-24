"""Encode a second quality from original RGB8 rasters; preserve every native command/result."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

p = argparse.ArgumentParser()
p.add_argument('binary', type=Path)
p.add_argument('catalog', type=Path)
p.add_argument('inputs', type=Path)
p.add_argument('output', type=Path)
p.add_argument('--quality', type=float, default=70)
p.add_argument('--input-prefix', default='')
p.add_argument('--strip-prefix', default='')
p.add_argument('--coarse-first', action='store_true')
a = p.parse_args()
a.output.mkdir(exist_ok=False)
catalog = json.loads(a.catalog.read_text())
inputs = json.loads(a.inputs.read_text())
records = []
for scene in catalog['datasets']:
    directory = a.output / scene['id']
    directory.mkdir()
    levels = []
    for level in (list(reversed(scene['levels'])) if a.coarse_first else scene['levels']):
        key, divisor = level['key'], level['divisor']
        reference = scene['representations']['lossless'][key]
        w, h = reference['width'], reference['height']
        source = inputs[scene['id']][key]
        paths = [
            Path(a.input_prefix + path[len(a.strip_prefix):])
            if a.strip_prefix and path.startswith(a.strip_prefix)
            else Path(path)
            for path in source['paths']
        ]
        before = [{'path': str(path), 'bytes': path.stat().st_size,
                   'sha256': hashlib.file_digest(path.open('rb'), 'sha256').hexdigest(),
                   'mtime_ns': path.stat().st_mtime_ns} for path in paths]
        cell = directory if divisor == 1 else directory / f'level-{divisor}'
        if divisor != 1:
            cell.mkdir()
            levels.append({'divisor': divisor, 'width': w, 'height': h})
        output = cell / ('full-lossy.imijset' if divisor == 1 else 'lossy.imijset')
        prefix = ['lossy-raw', str(w), str(h)] if len(paths) == 1 else ['lossy-mosaic', str(source['side'])]
        command = [str(a.binary), *prefix, *map(str, paths), str(output), str(a.quality)]
        record = {'scene': scene['id'], 'level': key, 'width': w, 'height': h,
                  'inputs': before, 'command': command,
                  'binary_sha256': hashlib.file_digest(a.binary.open('rb'), 'sha256').hexdigest()}
        (cell / 'command.json').write_text(json.dumps(record, indent=2) + '\n')
        with (cell / 'native.log').open('xb') as log:
            result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT)
        record['rc'] = result.returncode
        (cell / 'result.json').write_text(json.dumps(record, indent=2) + '\n')
        result.check_returncode()
        for item, path in zip(before, paths):
            if path.stat().st_size != item['bytes'] or path.stat().st_mtime_ns != item['mtime_ns']:
                raise RuntimeError('source raster changed during encoding')
        records.append(record)
        print(scene['id'], key, output.stat().st_size, flush=True)
    (directory / 'levels.json').write_text(json.dumps(sorted(levels,key=lambda level:level['divisor']), indent=2) + '\n')
(a.output / 'preparation.json').write_text(json.dumps(records, indent=2) + '\n')
