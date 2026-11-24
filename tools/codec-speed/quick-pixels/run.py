"""PR33 fresh-process comparison with the earlier stored-packet prototype."""
import gzip
import hashlib
import json
from pathlib import Path
import subprocess
import sys


def sha(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()


def execute(argv, path):
    path.with_suffix('.command.json').write_text(json.dumps([str(v) for v in argv]))
    with path.open('x') as log:
        rc = subprocess.run(argv, stdout=log, stderr=subprocess.STDOUT).returncode
    path.with_suffix('.rc').write_text(str(rc))
    if rc:
        raise ValueError((rc, path))


def main(probe, previous, fixtures, verified, zenbench, output):
    output.mkdir(parents=True)
    manifest = json.loads((fixtures / 'manifest.json').read_text())
    verified = json.loads(verified.read_text())
    prepared = []
    for f in manifest:
        geometry = output / (f['name'] + '.geometry')
        geometry.write_text(f"{f['width']} {f['height']} {dict(gray=1,ga=2,rgb=3,rgba=4)[f['channels']]} {dict(u8=1,u16=2,f32=4)[f['sample']]}\n")
        raw = fixtures / f['raw']
        if sha(raw) != f['sha256']:
            raise ValueError('input identity')
        for mode in ['wide'] + (['qoi'] if f['sample'] == 'u8' and f['channels'] in ('rgb', 'rgba') else []):
            path = output / f"{f['name']}-{mode}.bin"
            execute([probe, 'encode', raw, geometry, mode, path], path.with_suffix('.log'))
            found = [p for p in verified if p['image'] == f['name'] and p['mode'] == mode]
            if len(found) != 1 or sha(path) != found[0]['sha256']:
                raise ValueError('independently decoded file identity')
            prepared.append(found[0])
    (output / 'prepared.json').write_text(json.dumps(prepared, indent=2))
    identities = {'probe': sha(probe), 'previous': sha(previous), 'zenbench': sha(zenbench), 'manifest': manifest}
    (output / 'identities.json').write_text(json.dumps(identities, indent=2))
    def new(mode):
        return {'id': mode, 'program': str(probe), 'args': ['measure', '${raw}', '${geometry}', mode, '-'], 'timeout_seconds': 30, 'revision': sha(probe)}
    old = {'id': 'old-packets', 'program': str(previous), 'args': ['${raw}', '${geometry}', '2', '-'], 'timeout_seconds': 30, 'revision': sha(previous)}
    rgb = [f for f in manifest if f['sample'] == 'u8' and f['channels'] in ('rgb', 'rgba')]
    summary = []
    for group, baseline, candidate, images in [('all', old, new('wide'), manifest), ('rgb', new('qoi'), new('wide'), rgb)]:
        for action in ['encode', 'decode']:
            plan = {'schema': 1, 'baseline': baseline, 'candidates': [candidate],
                    'cases': [{'id': f['name'], 'inputs': {'raw': str(fixtures / f['raw']), 'geometry': str(output / (f['name'] + '.geometry'))}} for f in images],
                    'metric': {'extract': {'kind': 'prefix', 'prefix': action + '_ns='}, 'unit': 'ns', 'expected': 1, 'divide_by': 1},
                    'check': {'kind': 'json', 'prefix': 'check=', 'pointers': ['/raw_bytes']},
                    'warmup': 'One encode then four decodes; new core caller-buffer warmups, old prototype allocating warmups. Measured encode and allocating decode exclude IO and exact checks; new caller-buffer timing is retained separately.',
                    'blocks': 3, 'aa': True, 'deadline_seconds': 180}
            path = output / f'{group}-{action}.json'
            path.write_text(json.dumps(plan, indent=2))
            execute([zenbench, 'pair', path, '--output', output / f'{group}-{action}'], output / f'{group}-{action}.log')
            packet = json.loads(gzip.decompress((output / f'{group}-{action}/packet.json.gz').read_bytes()))
            if not packet['complete']:
                raise ValueError('incomplete measurement')
            summary.append({'group': group, 'action': action, 'observations': len(packet['invocations'])})
    (output / 'complete.json').write_text(json.dumps(summary, indent=2))


if __name__ == '__main__':
    main(*(Path(v).resolve() for v in sys.argv[1:]))
