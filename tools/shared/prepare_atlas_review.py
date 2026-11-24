#!/usr/bin/env python3
"""Add recognizable V4 examples, using the bundled sources and qualified WASI codec.

Keep every structural example and source; the UI initially shows review examples.
The input PNGs are decoded/cropped by the IMI tool, with no image resampling.
"""
import argparse
import base64
import json
from pathlib import Path
import subprocess


def prepare(catalog, wasm, output):
    text = catalog.read_text()
    data = json.loads(text.removeprefix('window.ATLAS=').strip().removesuffix(';'))
    sources = {s['name']: s for s in data['sources']}
    photo = sources['1407']
    plot = next(s for s in data['sources'] if s['name'].startswith('7000_plots_'))
    common = ['--crop', '512x512', '--channels', 'rgb', '--sample', 'u8']
    presets = [
        ('Coast photograph — lossless', photo, ['--lossless', '--effort', '2']),
        ('Coast photograph — lossy ANS Q85', photo, ['--lossy', '--quality', '85', '--entropy', 'runlevel']),
        ('Coast photograph — lossy SplitHuf Q85', photo, ['--lossy', '--quality', '85', '--entropy', 'splithuf']),
        ('Coast photograph — lossy sampled Q85', photo, ['--lossy', '--quality', '85', '--layout-2', 'dct8,both', '--layout-3', 'dct8,both']),
        ('Coast photograph — lossy small Q85', photo, ['--lossy', '--quality', '85', '--layout-1', 'adaptive4,full']),
        ('Coast photograph — Quick exact', photo, ['--quick']),
        ('Line plot — lossless', plot, ['--lossless', '--effort', '2']),
        ('Line plot — lossy ANS Q90', plot, ['--lossy', '--quality', '90', '--entropy', 'runlevel']),
    ]

    def run(command, payload, flags=()):
        p = subprocess.run(['wasmtime', 'run', str(wasm), command, *flags],
                           input=payload, capture_output=True, check=True)
        return p.stdout

    assert not output.exists(), output
    for title, source, mode in presets:
        png = base64.b64decode(source['png'])
        encoded = run('encode', png, common + mode)
        trace = json.loads(run('trace', encoded))
        assert trace['decode'] == 'ok' and not trace['checks']['problems'] and not trace['checks']['unattributed']
        version = next(n['value'] for n in trace['nodes'] if n['label'] == 'header version')
        assert version.startswith('4 ='), version
        rgba = run('decode-rgba', encoded)
        assert len(rgba) == trace['image']['width'] * trace['image']['height'] * 4
        if '--lossless' in mode or '--quick' in mode:
            raw = run('convert-raw', png, common + ['--lossless'])
            expected = b''.join(raw[i:i + 3] + b'\xff' for i in range(0, len(raw), 3))
            assert rgba == expected
        trace.update(name='V4 · ' + title, review=True,
                     source={'name': source['name'], 'crop': 'center 512x512', 'resampled': False},
                     rgba=base64.b64encode(rgba).decode())
        data['files'].append(trace)
        print(title, len(encoded), 'bytes; complete trace and decoded pixels', flush=True)
    photo['title'] = 'Coast photograph · imazen-26/1407'
    plot['title'] = 'Line plot · imazen-26/7000'
    data['sources'] = [photo, plot] + [s for s in data['sources'] if s not in (photo, plot)]
    data['reviewGallery'] = True
    output.write_text('window.ATLAS=' + json.dumps(data) + ';\n')


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('catalog', type=Path)
    p.add_argument('wasm', type=Path)
    p.add_argument('output', type=Path)
    a = p.parse_args()
    prepare(a.catalog, a.wasm, a.output)
