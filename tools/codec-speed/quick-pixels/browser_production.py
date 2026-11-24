"""Use the existing browser probes on retained real-image production files."""
import json
from pathlib import Path
import shutil
import sys

from pair import execute, sha


def main(web, fixtures, native, wasm, production, output, bit_smarts=False):
    output.mkdir()
    data = output / 'fixtures'
    data.mkdir()
    manifest = json.loads((fixtures / 'manifest.json').read_text())
    prepared = json.loads((production / 'prepared.json').read_text())
    entries = []
    modes = ('quick', 'quick-packed', 'quick-bits', '0', '7.5') if bit_smarts else ('quick', '0', '7.5')
    for image in manifest:
        for mode in modes:
            arm = f'imij-{mode}-64-1'
            record = next(p for p in prepared if p['image'] == image['name'] and p['arm'] == arm)
            name = f"{image['name']}-{mode}.imi"
            source = production / f"{image['name']}-{arm}.bin"
            if sha(source) != record['sha256']:
                raise ValueError('retained file changed')
            shutil.copyfile(source, data / name)
            entries.append({**image, 'name': f"{image['name']}/{mode}", 'encoded': name})
    path = data / 'manifest.json'
    path.write_text(json.dumps(entries, indent=2))
    for block in range(3):
        execute(['node', web / 'scripts/measure-browser.mjs', web, 'candidate', path, 'chromium'], output / f'decode-{block}.log')
    plan = output / 'encode-plan.json'
    configurations = [{'mode': 'quick', 'effort': 0}]
    if bit_smarts:
        configurations += [{'mode':'quick','effort':0,'bitSmarts':v} for v in ('packed','packets')]
    configurations += [{'mode':'lossless','effort':0}, {'mode':'lossless','effort':7.5}]
    plan.write_text(json.dumps(configurations))
    execute(['node', web / 'scripts/measure-encode.mjs', web, fixtures, native, wasm, output / 'encode', plan], output / 'encode.log')
    encode = json.loads((output / 'encode/summary.json').read_text())
    if not encode['complete'] or encode['cells'] != len(manifest) * len(configurations):
        raise ValueError('incomplete encoder coverage')
    observations = []
    for block in range(3):
        lines = (output / f'decode-{block}.log').read_text().splitlines()
        value = json.loads(next(line.removeprefix('observations=') for line in lines if line.startswith('observations=')))
        if len(value['rows']) != len(entries) * 10:
            raise ValueError('incomplete browser decode domain')
        observations.append(value)
    (output / 'complete.json').write_text(json.dumps({'decode_processes': 3, 'decode_observations': sum(len(v['rows']) for v in observations), 'encode_cells': encode['cells'], 'encode_ms': encode['totalEncodeMs'], 'browser': observations[0]['version']}, indent=2))


if __name__ == '__main__':
    args = sys.argv[1:]
    bit_smarts = args[-1:] == ['--bit-smarts']
    if bit_smarts: args.pop()
    main(*(Path(v).resolve() if i != 3 else v for i, v in enumerate(args)), bit_smarts=bit_smarts)
