#!/usr/bin/env python3
"""Recreate measured encode cells and their exact native reconstructed rasters."""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('binary', type=Path)
parser.add_argument('fixtures', type=Path)
parser.add_argument('measurements', type=Path)
parser.add_argument('output', type=Path)
args = parser.parse_args()
args.output.mkdir()
rows = json.loads(args.measurements.read_text())['rows']
manifest = []
sha = lambda b: hashlib.sha256(b).hexdigest()
for i, row in enumerate(rows):
    command = [str(args.binary), 'encode', '--raw', str(row['width']), str(row['height']),
               row.get('channels', 'rgb'), row.get('sample', 'u8'), '--' + row['mode'], '--effort', str(row['effort'])]
    if 'quality' in row:
        command += ['--quality', str(row['quality'])]
    pixels = (args.fixtures / row['image']).read_bytes()
    encoded = subprocess.run(command, input=pixels, capture_output=True, check=True)
    if sha(encoded.stdout) != row['encodedSha']:
        raise ValueError('encoded output differs from measured cell')
    decoded = subprocess.run([str(args.binary), 'decode-raw'], input=encoded.stdout,
                             capture_output=True, check=True)
    if sha(decoded.stdout) != row['decodedSha']:
        raise ValueError('native reconstructed raster differs')
    name = f"{Path(row['image']).stem}-{row['mode']}-e{row['effort']}-q{row.get('quality', 'na')}"
    for suffix, data in [('.imi', encoded.stdout), ('.raw', decoded.stdout),
                         ('.encode.log', encoded.stderr), ('.decode.log', decoded.stderr)]:
        (args.output / (name + suffix)).write_bytes(data)
    manifest.append(dict(name=name, encoded=name + '.imi', raw=name + '.raw',
                         width=row['width'], height=row['height'], sha256=row['decodedSha'],
                         encodedSha256=row['encodedSha'], sourceSha256=sha(pixels), command=command))
(args.output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps(dict(files=len(manifest), binarySha256=sha(args.binary.read_bytes()))))
