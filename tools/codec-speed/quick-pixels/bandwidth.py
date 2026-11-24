"""Cache-size qualification on tiled original F32 bytes; no image-quality claim."""
import hashlib
import json
from pathlib import Path
import sys
from pair import execute, sha


def main(probe, fixtures, zenbench, output, mode='pooled'):
    if mode not in ('pooled', 'direct'):
        raise ValueError('mode')
    output.mkdir()
    f = next(f for f in json.loads((fixtures / 'manifest.json').read_text()) if f['name'] == 'era5-temperature')
    source = (fixtures / f['raw']).read_bytes()
    if hashlib.sha256(source).hexdigest() != f['sha256']:
        raise ValueError('source identity')
    cases = []
    sizes = (45, 180, 721, 2884, 11536, 23072) if mode == 'pooled' else (1, 45, 180, 360, 721, 1082, 1443, 2164, 2884, 5768, 11536, 23072)
    for rows in sizes:
        name = f'rows-{rows}'
        n = f['width'] * rows * 4
        data = (source * ((n + len(source) - 1) // len(source)))[:n]
        raw = output / (name + '.raw')
        raw.write_bytes(data)
        geometry = output / (name + '.geometry')
        geometry.write_text(f"{f['width']} {rows} 1 4\n")
        encoded = output / (name + '.imi')
        execute([probe, 'prepare', 'imij-quick', '1', '64', raw, geometry, encoded], output / (name + '.prepare.log'))
        cases.append({'id': name, 'inputs': {'raw': str(raw), 'geometry': str(geometry), 'encoded': str(encoded)},
                      'bytes': n, 'sha256': sha(raw), 'encoded_sha256': sha(encoded)})
    def arm(action, threads):
        return {'id': f'{action}-{threads}', 'program': str(probe), 'revision': sha(probe), 'timeout_seconds': 60,
                'args': [action, 'imij-quick', str(threads), '64', '${raw}', '${geometry}', '${encoded}']}
    actions = ('decode-file', 'decode-into-file', 'copy-file') if mode == 'pooled' else ('decode-direct-file', 'decode-into-direct-file')
    arms = [arm(a, t) for a in actions for t in (1, 2, 4, 8)]
    plan = {'schema': 1, 'baseline': arms[0], 'candidates': arms[1:],
            'cases': [{'id': c['id'], 'inputs': c['inputs']} for c in cases],
            'metric': {'extract': {'kind': 'prefix', 'prefix': 'decode_ns='}, 'unit': 'ns', 'expected': 1, 'divide_by': 1},
            'check': {'kind': 'json', 'prefix': 'check=', 'pointers': ['/raw_sha256']},
            'warmup': f'Three warmups, ten calls, fresh processes; {mode} caller context. Original F32 rows are truncated/repeated only to qualify cache and bandwidth sizes, not natural-image compression.',
            'blocks': 3, 'aa': True, 'deadline_seconds': 900}
    (output / 'identities.json').write_text(json.dumps({'source': f, 'mode': mode, 'program_sha256': sha(probe), 'cases': cases}, indent=2))
    path = output / 'plan.json'
    path.write_text(json.dumps(plan, indent=2))
    execute([zenbench, 'pair', path, '--output', output / 'paired'], output / 'paired.log')


if __name__ == '__main__':
    main(*(Path(v).resolve() for v in sys.argv[1:5]), *sys.argv[5:])
