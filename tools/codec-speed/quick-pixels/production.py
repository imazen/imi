"""Paired production Pareto study: exact files, real inputs, three fresh blocks."""
import gzip
import json
from pathlib import Path
import sys
from pair import execute, independent, sha


def main(probe, oracle, fixtures, zenbench, output, before=None, bit_smarts=False):
    output.mkdir(parents=True)
    manifest = json.loads((fixtures/'manifest.json').read_text())
    prepared = []
    variants = [('imij-quick', t, 64) for t in (1, 2, 4, 8)] + [(f'imij-{e}', t, 64) for e in (0, 1, 7.5) for t in (1, 2, 4, 8)]
    if bit_smarts:
        variants = [(c,t,64) for c in ('imij-quick','imij-quick-packed','imij-quick-bits','imij-0','imij-7.5') for t in (1,2,4,8)]
    if before is not None:
        variants += [('imij-before', t, 64) for t in (1, 2, 4, 8)]
    qoi = [('zenbitmaps', 1, 0)] + [('zenbitmaps', t, 64) for t in (1, 2, 4, 8)]
    def arm_id(c, t, r):
        return f'{c}-{r}-{t}'
    total_encode = 0
    for f in manifest:
        raw = fixtures/f['raw']
        if sha(raw) != f['sha256']:
            raise ValueError('raw identity')
        geometry = output/(f['name']+'.geometry')
        geometry.write_text(f"{f['width']} {f['height']} {dict(gray=1,ga=2,rgb=3,rgba=4)[f['channels']]} {dict(u8=1,u16=2,f32=4)[f['sample']]}\n")
        configs = variants + (qoi if f['sample'] == 'u8' and f['channels'] in ('rgb', 'rgba') else [])
        identities = {}
        for c, t, r in configs:
            name = arm_id(c, t, r)
            path = output/(f['name']+'-'+name+'.bin')
            log = path.with_suffix('.log')
            execute([before if c == 'imij-before' else probe, 'prepare', 'imij-quick' if c == 'imij-before' else c, str(t), str(r), raw, geometry, path], log)
            total_encode += sum(int(s.split('=')[1]) for s in log.read_text().splitlines() if s.startswith('encode_total_ns='))
            if total_encode > 120e9:
                raise ValueError('encode budget exceeded during preparation')
            if c.startswith('imij'):
                decoded = path.with_suffix('.oracle.raw')
                execute([oracle, 'decode', path, decoded], path.with_suffix('.oracle.log'))
                if sha(decoded) != f['sha256']:
                    raise ValueError('independent IMIJ pixels')
            else:
                independent(path, f, r, False)
            key = c, r
            digest = sha(path)
            if key in identities and identities[key] != digest:
                raise ValueError('thread-dependent encoding')
            identities[key] = digest
            prepared.append({'image': f['name'], 'arm': name, 'sha256': digest, 'bytes': path.stat().st_size})
    if before is not None:
        for f in manifest:
            for t in (1, 2, 4, 8):
                old = next(p for p in prepared if p['image'] == f['name'] and p['arm'] == arm_id('imij-before', t, 64))
                new = next(p for p in prepared if p['image'] == f['name'] and p['arm'] == arm_id('imij-quick', t, 64))
                if old['sha256'] != new['sha256']:
                    raise ValueError('optimization changed encoded bytes')
    (output/'prepared.json').write_text(json.dumps(prepared, indent=2))
    shared = [f for f in manifest if f['sample'] == 'u8' and f['channels'] in ('rgb', 'rgba')]
    other = [f for f in manifest if f not in shared]
    (output/'identities.json').write_text(json.dumps({'probe': sha(probe), 'before_probe': sha(before) if before else None, 'oracle': sha(oracle), 'zenbench': sha(zenbench), 'manifest': manifest, 'groups': {'qoi-domain': variants+qoi, 'other-formats': variants}}, indent=2))
    for group, cases, configs in [('qoi-domain', shared, variants+qoi), ('other-formats', other, variants)]:
        for action in ('encode', 'decode'):
            def arm(c, t, r):
                name = arm_id(c, t, r)
                key = name.replace('-', '_').replace('.', '_')
                program = before if c == 'imij-before' else probe
                codec = 'imij-quick' if c == 'imij-before' else c
                return {'id': name, 'program': str(program), 'revision': sha(program), 'timeout_seconds': 30,
                        'args': [action if action == 'encode' else 'decode-file', codec, str(t), str(r), '${raw}', '${geometry}', '-' if action == 'encode' else '${'+key+'}']}
            inputs = []
            for f in cases:
                data = {'raw': str(fixtures/f['raw']), 'geometry': str(output/(f['name']+'.geometry'))}
                data.update({arm_id(*v).replace('-', '_').replace('.', '_'): str(output/(f['name']+'-'+arm_id(*v)+'.bin')) for v in configs})
                inputs.append({'id': f['name'], 'inputs': data})
            plan = {'schema': 1, 'baseline': arm(*configs[0]), 'candidates': [arm(*v) for v in configs[1:]], 'cases': inputs,
                    'metric': {'extract': {'kind': 'prefix', 'prefix': action+'_ns='}, 'unit': 'ns', 'expected': 1, 'divide_by': 1},
                    'check': {'kind': 'json', 'prefix': 'check=', 'pointers': ['/raw_sha256']},
                    'warmup': 'Encode: one warmup then one timed allocating encode. Decode: retained independently decoded file; cross-check, three warmups then ten timed allocating decodes. Thread-pool creation and IO excluded; adaptive IMIJ native decoder launches remain timed, quick mode reuses the current pool.',
                    'blocks': 3, 'aa': True, 'deadline_seconds': 600}
            path = output/(group+'-'+action+'.json')
            path.write_text(json.dumps(plan, indent=2))
            execute([zenbench, 'pair', path, '--output', output/(group+'-'+action)], path.with_suffix('.log'))
            packet = json.loads(gzip.decompress((output/(group+'-'+action)/'packet.json.gz').read_bytes()))
            if not packet['complete']:
                raise ValueError('incomplete paired run')
            for child in packet['invocations']:
                stdout = output/(group+'-'+action)/Path(child['output']).name/'stdout.txt'
                total_encode += sum(int(s.split('=')[1]) for s in stdout.read_text().splitlines() if s.startswith('encode_total_ns='))
            (output/'encode-budget.json').write_text(json.dumps({'encode_ns': total_encode, 'limit_ns': 120_000_000_000}))
            if total_encode > 120e9:
                raise ValueError('encode budget exceeded')
    (output/'complete.json').write_text(json.dumps({'prepared_files': len(prepared), 'encode_seconds': total_encode/1e9}))


if __name__ == '__main__':
    args = sys.argv[1:]
    bit_smarts = args[-1:] == ['--bit-smarts']
    if bit_smarts:
        args.pop()
    main(*(Path(v).resolve() for v in args), bit_smarts=bit_smarts)
