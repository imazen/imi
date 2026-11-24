#!/usr/bin/env python3
"""Freeze real raw/PNG/PGM encode inputs from a hashed, explicit study plan.

PNG decoding/conversion uses the existing command engine (zenpng). PGM only
unpacks its big-endian U16 storage; no sample/color conversion is performed.
"""
import argparse
import hashlib
import json
import re
import subprocess
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('binary', type=Path)
p.add_argument('plan', type=Path)
p.add_argument('output', type=Path)
a = p.parse_args()
a.output.mkdir()
manifest = []
for item in json.loads(a.plan.read_text()):
    source = Path(item['source']).read_bytes()
    if hashlib.sha256(source).hexdigest() != item['sourceSha256']:
        raise ValueError('source hash mismatch: ' + item['source'])
    w, h = item['width'], item['height']
    if item['storage'] == 'png':
        cmd = [str(a.binary), 'convert-raw', '--channels', item['channels'], '--sample', item['sample']]
        result = subprocess.run(cmd, input=source, capture_output=True, check=True)
        (a.output / (item['name'] + '.conversion.log')).write_bytes(result.stderr)
        info = json.loads(result.stderr)
        if (info['w'], info['h']) != (w, h):
            raise ValueError('PNG geometry mismatch')
        raw = result.stdout
    elif item['storage'] == 'pgm16':
        header = re.match(rb'P5\s+(\d+)\s+(\d+)\s+65535\s', source)
        if not header or tuple(map(int, header.groups())) != (w, h):
            raise ValueError('unexpected PGM header')
        data = source[header.end():]
        raw = bytearray(len(data))
        raw[0::2], raw[1::2] = data[1::2], data[0::2]
    elif item['storage'] == 'raw':
        raw = source
    else:
        raise ValueError('unknown source storage')
    channels = {'gray': 1, 'ga': 2, 'rgb': 3, 'rgba': 4}[item['channels']]
    size = {'u8': 1, 'u16': 2, 'f32': 4}[item['sample']]
    if len(raw) != w * h * channels * size:
        raise ValueError('raw length mismatch')
    name = item['name'] + '.raw'
    (a.output / name).write_bytes(raw)
    manifest.append(dict(item, raw=name, sha256=hashlib.sha256(raw).hexdigest()))
(a.output / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps(dict(inputs=len(manifest), binarySha256=hashlib.sha256(a.binary.read_bytes()).hexdigest())))
