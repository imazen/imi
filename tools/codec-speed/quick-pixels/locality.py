"""Separate quick decode allocation, packet work and band-copy scheduling."""
import json
from pathlib import Path
import sys
from pair import execute, sha


def main(old, new, fixtures, retained, zenbench, output, mode='pooled'):
    if mode not in ('pooled', 'direct'):
        raise ValueError('mode')
    output.mkdir()
    manifest = json.loads((fixtures / 'manifest.json').read_text())
    prepared = json.loads((retained / 'prepared.json').read_text())
    actions = ('decode-file', 'decode-into-file', 'copy-file') if mode == 'pooled' else ('decode-direct-file', 'decode-into-direct-file')
    arms = [(program, action, threads) for program in (old, new)
            for action in actions for threads in (1, 2, 4, 8)]
    for f in manifest:
        raw = fixtures / f['raw']
        if sha(raw) != f['sha256']:
            raise ValueError('original raw identity')
        for threads in (1, 2, 4, 8):
            reference = next(p for p in prepared if p['image'] == f['name'] and p['arm'] == f'imij-quick-64-{threads}')
            dest = output / f"{f['name']}-{threads}.imi"
            execute([new, 'prepare', 'imij-quick', str(threads), '64', raw, retained / (f['name'] + '.geometry'), dest], dest.with_suffix('.log'))
            if sha(dest) != reference['sha256']:
                raise ValueError('candidate changed encoded bytes')
    def arm(program, action, threads):
        return {'id': f'{"old" if program == old else "new"}-{action}-{threads}', 'program': str(program), 'revision': sha(program),
                'args': [action, 'imij-quick', str(threads), '64', '${raw}', '${geometry}', '${encoded}'], 'timeout_seconds': 30}
    plan = {'schema': 1, 'baseline': arm(*arms[0]), 'candidates': [arm(*a) for a in arms[1:]],
            'cases': [{'id': f['name'], 'inputs': {'raw': str(fixtures / f['raw']), 'geometry': str(retained / (f['name'] + '.geometry')),
                       'encoded': str(output / f"{f['name']}-1.imi")}} for f in manifest],
            'metric': {'extract': {'kind': 'prefix', 'prefix': 'decode_ns='}, 'unit': 'ns', 'expected': 1, 'divide_by': 1},
            'check': {'kind': 'json', 'prefix': 'check=', 'pointers': ['/raw_sha256']},
            'warmup': f'Three warmups, ten calls per observation. {mode} caller context. Pool construction, IO and hashes excluded.',
            'blocks': 3, 'aa': True, 'deadline_seconds': 600}
    (output / 'identities.json').write_text(json.dumps({'old': sha(old), 'new': sha(new), 'zenbench': sha(zenbench), 'manifest': manifest}, indent=2))
    path = output / 'plan.json'
    path.write_text(json.dumps(plan, indent=2))
    execute([zenbench, 'pair', path, '--output', output / 'paired'], output / 'paired.log')


if __name__ == '__main__':
    main(*(Path(v).resolve() for v in sys.argv[1:7]), *sys.argv[7:])
