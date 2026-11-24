"""Byte-exact quick-pixel optimization comparison on the frozen real inputs."""
import json
from pathlib import Path
import sys
from pair import execute, sha


def main(old, new, retained, zenbench, output):
    output.mkdir(parents=True)
    ids = json.loads((retained / 'identities.json').read_text())
    manifest = ids['manifest']
    prepared = json.loads((retained / 'prepared.json').read_text())
    fixtures = Path(json.loads((retained / 'encode.json').read_text())['cases'][0]['inputs']['raw']).parent
    for f in manifest:
        if sha(fixtures / f['raw']) != f['sha256']:
            raise ValueError('source pixels')
    for p in prepared:
        f = next(f for f in manifest if f['name'] == p['image'])
        codec, rows, threads = p['arm'].split('-')
        dest = output / (p['image'] + '-' + p['arm'] + '.bin')
        execute([new, 'prepare', codec, threads, rows, fixtures / f['raw'], retained / (p['image'] + '.geometry'), dest], dest.with_suffix('.log'))
        if sha(dest) != p['sha256']:
            raise ValueError(('encoded bytes changed', p))
    shared = [f for f in manifest if f['sample'] == 'u8' and f['channels'] in ('rgb', 'rgba')]
    arms = [(p, r, t) for r, t in [(0, 1), (64, 1), (64, 8)] for p in [old, new]]
    (output / 'identities.json').write_text(json.dumps({'old': sha(old), 'new': sha(new), 'zenbench': sha(zenbench), 'retained': sha(retained / 'prepared.json'), 'arms': [[str(p), r, t] for p, r, t in arms]}, indent=2))
    for action in ['encode', 'decode']:
        def arm(p, r, t):
            return {'id': f'{"old" if p == old else "new"}-{r}-{t}', 'program': str(p), 'args': [action, 'original', str(t), str(r), '${raw}', '${geometry}', '-'], 'revision': sha(p), 'timeout_seconds': 30}
        plan = {'schema': 1, 'baseline': arm(*arms[0]), 'candidates': [arm(*a) for a in arms[1:]],
                'cases': [{'id': f['name'], 'inputs': {'raw': str(fixtures/f['raw']), 'geometry': str(retained/(f['name']+'.geometry'))}} for f in shared],
                'metric': {'extract': {'kind': 'prefix', 'prefix': action+'_ns='}, 'unit': 'ns', 'expected': 1, 'divide_by': 1},
                'check': {'kind': 'json', 'prefix': 'check=', 'pointers': ['/raw_sha256']},
                'warmup': 'Identical quick_pair warmups/measurement; all 82 retained encoded files required byte-identical first.',
                'blocks': 3, 'aa': True, 'deadline_seconds': 180}
        path = output / (action+'.json')
        path.write_text(json.dumps(plan, indent=2))
        execute([zenbench, 'pair', path, '--output', output/action], output/(action+'.log'))


if __name__ == '__main__':
    main(*(Path(v).resolve() for v in sys.argv[1:]))
