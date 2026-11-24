"""Frozen real inputs; PR33 three blocks/A-A. Experiment only, not IMIJ files."""
import gzip
import hashlib
import json
import pathlib
import subprocess
import sys
from oracle import decode


def sha(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()


def execute(argv, path):
    path.with_suffix('.command.json').write_text(json.dumps(argv))
    with path.open('x') as out:
        r = subprocess.run(argv, stdout=out, stderr=subprocess.STDOUT)
    path.with_suffix('.rc').write_text(str(r.returncode))
    if r.returncode:
        raise RuntimeError(path)


def main(probe, fixtures, zenbench, output):
    output.mkdir()
    manifest = json.loads((fixtures / 'manifest.json').read_text())
    channels = {'gray': 1, 'ga': 2, 'rgb': 3, 'rgba': 4}
    sizes = {'u8': 1, 'u16': 2, 'f32': 4}
    for f in manifest:
        (output / (f['name'] + '.geometry')).write_text(f"{f['width']} {f['height']} {channels[f['channels']]} {sizes[f['sample']]}\n")
    prepared = []
    for image in manifest:
        raw = fixtures / image['raw']
        if sha(raw) != image['sha256']:
            raise ValueError('raw hash')
        for mode in range(6):
            if mode == 4 and (image['channels'], image['sample']) != ('rgb', 'u8'):
                continue
            target = output / f"{image['name']}-{mode}.spx"
            argv = [str(probe), str(raw), str(image['width']), str(image['height']), str(channels[image['channels']]), str(sizes[image['sample']]), str(mode), str(target)]
            execute(argv, target.with_suffix('.log'))
            if decode(target.read_bytes()) != raw.read_bytes():
                raise ValueError(f'independent pixels {target}')
            prepared.append({'image': image['name'], 'mode': mode, 'bytes': target.stat().st_size, 'sha256': sha(target), 'source_sha256': sha(raw)})
    (output / 'prepared.json').write_text(json.dumps(prepared, indent=2))
    (output / 'identities.json').write_text(json.dumps({'probe': sha(probe), 'zenbench': sha(zenbench), 'manifest': manifest}, indent=2))
    summary = []
    for group, modes, images in [('all', [0, 1, 2, 3, 5], manifest), ('green', [0, 4], [f for f in manifest if (f['channels'], f['sample']) == ('rgb', 'u8')])]:
        cases = [{'id': f['name'], 'inputs': {'raw': str(fixtures / f['raw']), 'geometry': str(output / (f['name'] + '.geometry'))}} for f in images]
        arms = [{'id': f'mode{mode}', 'program': str(probe), 'args': ['${raw}', '${geometry}', str(mode), '-'], 'timeout_seconds': 30, 'revision': sha(probe)} for mode in modes]
        for action in ['encode', 'decode']:
            plan = {'schema': 1, 'baseline': arms[0], 'candidates': arms[1:], 'cases': cases, 'metric': {'extract': {'kind': 'prefix', 'prefix': action + '_ns='}, 'unit': 'ns', 'expected': 1, 'divide_by': 1}, 'check': {'kind': 'json', 'prefix': 'check=', 'pointers': ['/raw_bytes']}, 'warmup': 'One encode and four allocating decodes before measurement; input/output I/O and exact checks outside timers. Both encode/decode measured once per child.', 'blocks': 3, 'aa': True, 'deadline_seconds': 180}
            path = output / f'{group}-{action}.json'
            path.write_text(json.dumps(plan, indent=2))
            execute([str(zenbench), 'pair', str(path), '--output', str(output / f'{group}-{action}')], output / f'{group}-{action}.log')
            packet = json.loads(gzip.decompress((output / f'{group}-{action}' / 'packet.json.gz').read_bytes()))
            if not packet['complete']:
                raise ValueError('incomplete measurement')
            summary.append({'group': group, 'action': action, 'observations': len(packet['invocations'])})
    (output / 'complete.json').write_text(json.dumps(summary, indent=2))


if __name__ == '__main__':
    main(*(pathlib.Path(s).resolve() for s in sys.argv[1:]))
