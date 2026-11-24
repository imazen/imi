#!/usr/bin/env python3
"""Fixed-work browser comparison; exact native hashes remain mandatory."""
import json
from pathlib import Path
import subprocess
import sys

web, baseline, candidate, manifest, destination, engines, counts, variant = sys.argv[1:]
out = Path(destination)
out.mkdir()
fixtures = json.loads(Path(manifest).read_text())
if not isinstance(fixtures, list) or not fixtures:
    raise RuntimeError('nonempty fixture domain required')
expected = {(f['id'], trial, int(count)) for f in fixtures for trial in range(3) for count in counts.split(',')}
results = []
for engine in engines.split(','):
    arms = []
    for name, url in [('before', baseline), ('after', candidate)]:
        folder = out / f'{engine}-{name}'
        command = ['node', 'tools/imij-wasm/web/scripts/check-worker-scaling.mjs',
                   web, url, manifest, str(folder), engine, 'regions', counts, variant]
        with (out / 'commands.jsonl').open('a') as log:
            log.write(json.dumps(command) + '\n')
        subprocess.run(command, check=True)
        rows = json.loads((folder / 'results.json').read_text())['results']
        keyed = {(r['fixture'], r['trial'], r['workers']): r for r in rows}
        if len(rows) != len(expected) or set(keyed) != expected:
            raise RuntimeError(f'incomplete product: {folder}')
        arms.append(keyed)
    for key, before in arms[0].items():
        after = arms[1][key]
        if not before['pixelHash'] or before['pixelHash'] != after['pixelHash']:
            raise RuntimeError(f'pixel mismatch: {engine}/{key}')
    for f in fixtures:
        for count in map(int, counts.split(',')):
            times = [max(r['elapsedMs'] for r in arm.values() if r['fixture'] == f['id'] and r['workers'] == count) for arm in arms]
            results.append({'engine': engine, 'fixture': f['id'], 'workers': count,
                            'beforeWorstMs': times[0], 'afterWorstMs': times[1],
                            'ratio': times[1] / times[0]})
    (out / 'results.json').write_text(json.dumps(results, indent=2))
    print(f'{engine}: {len(expected)} exact pairs PASS', flush=True)
