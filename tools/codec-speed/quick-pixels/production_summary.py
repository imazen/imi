"""Verify production PR33 packets, then plot measured three-objective frontiers."""
import gzip
import hashlib
import json
import math
from pathlib import Path
import sys
from pair_summary import require


def main(root, output):
    output.mkdir()
    ids = json.loads((root / 'identities.json').read_text())
    prepared = json.loads((root / 'prepared.json').read_text())
    require((root / 'complete.json').is_file(), 'completed production run')
    for p in prepared:
        path = root / f"{p['image']}-{p['arm']}.bin"
        require(path.stat().st_size == p['bytes'], 'encoded length')
        require(hashlib.sha256(path.read_bytes()).hexdigest() == p['sha256'], 'encoded hash')
    shared = [f for f in ids['manifest'] if f['sample'] == 'u8' and f['channels'] in ('rgb', 'rgba')]
    other = [f for f in ids['manifest'] if f not in shared]
    times, aa, count = {}, [], 0
    for group, manifest in [('qoi-domain', shared), ('other-formats', other)]:
        arms = [f'{c}-{r}-{t}' for c, t, r in ids['groups'][group]]
        for action in ('encode', 'decode'):
            directory = root / f'{group}-{action}'
            packet = json.loads(gzip.decompress((directory / 'packet.json.gz').read_bytes()))
            require(packet['complete'] and packet['plan']['blocks'] == 3 and packet['plan']['aa'], 'protocol')
            require(len(packet['invocations']) == len(manifest) * (len(arms) + 1) * 3, 'count')
            seen, cells = set(), {}
            for v in packet['invocations']:
                key = v['case'], v['arm'], v['block']
                require(key not in seen, 'duplicate')
                seen.add(key)
                require(v['status'] == 'completed' and v['exit_code'] == 0 and v['continuation_safe'], 'child status')
                require(len(v['values_ns']) == 1 and math.isfinite(v['values_ns'][0]) and v['values_ns'][0] > 0, 'time')
                require(v['correctness'] == [manifest[v['case']]['sha256']], 'original pixel digest')
                for path, digest in v['artifacts'].items():
                    local = directory / Path(path).parent.name / Path(path).name
                    require(hashlib.sha256(local.read_bytes()).hexdigest() == digest, 'child artifact hash')
                stdout = (directory / Path(v['output']).name / 'stdout.txt').read_text()
                result = json.loads(next(s[7:] for s in stdout.splitlines() if s.startswith('result=')))
                arm = arms[v['arm']] if v['arm'] < len(arms) else arms[0]
                reference = next(p for p in prepared if p['image'] == manifest[v['case']]['name'] and p['arm'] == arm)
                require(result['encoded_sha256'] == reference['sha256'] and result['bytes'] == reference['bytes'], 'retained encoded identity')
                cells.setdefault(key[:2], []).append(v['values_ns'][0])
            require(all(len(v) == 3 for v in cells.values()), 'three observations')
            require(all(v['complete_blocks'] == 3 and v['correctness_checked'] for v in packet['summaries']), 'checked blocks')
            for i, f in enumerate(manifest):
                aa.append({'image': f['name'], 'action': action, 'ratio': max(cells[i, len(arms)]) / max(cells[i, 0])})
                for j, arm in enumerate(arms):
                    times[f['name'], arm, action] = max(cells[i, j])
            count += len(packet['invocations'])
    rows = []
    for p in prepared:
        f = next(f for f in ids['manifest'] if f['name'] == p['image'])
        row = {**p, 'category': f['category'], 'pixels': f['width'] * f['height'], 'threads': int(p['arm'].rsplit('-', 1)[1])}
        for action in ('encode', 'decode'):
            row[action + '_ns'] = times[f['name'], p['arm'], action]
        rows.append(row)
    aggregate = []
    for category in dict.fromkeys(r['category'] for r in rows):
        for arm in dict.fromkeys(r['arm'] for r in rows if r['category'] == category):
            cells = [r for r in rows if r['category'] == category and r['arm'] == arm]
            pixels = sum(c['pixels'] for c in cells)
            aggregate.append({'category': category, 'arm': arm, 'threads': cells[0]['threads'], 'control': arm.startswith('imij-before-'),
                              'bpp': 8 * sum(c['bytes'] for c in cells) / pixels,
                              **{a + '_mpx_s': pixels * 1000 / sum(c[a + '_ns'] for c in cells) for a in ('encode', 'decode')}})
    # A frontier is descriptive of these observations, not a statistical significance claim.
    for row in aggregate:
        peers = [r for r in aggregate if not r['control'] and (r['category'], r['threads']) == (row['category'], row['threads'])]
        row['frontier'] = not any(
            p['bpp'] <= row['bpp'] and p['encode_mpx_s'] >= row['encode_mpx_s'] and p['decode_mpx_s'] >= row['decode_mpx_s']
            and (p['bpp'], p['encode_mpx_s'], p['decode_mpx_s']) != (row['bpp'], row['encode_mpx_s'], row['decode_mpx_s']) for p in peers)
    optimization = []
    for old in aggregate:
        if old['control']:
            new = next(r for r in aggregate if r['category'] == old['category'] and r['arm'] == old['arm'].replace('imij-before-', 'imij-quick-'))
            optimization.append({'category': old['category'], 'threads': old['threads'], **{a+'_time_ratio': old[a+'_mpx_s']/new[a+'_mpx_s'] for a in ('encode', 'decode')}})
    result = {'optimization': optimization, 'observations': count, 'rows': rows, 'aggregate': aggregate, 'aa': aa,
              'method': 'Slowest of three fresh-process blocks. Total pixels / sum of per-image times. Frontier compares all three objectives at equal thread count.'}
    (output / 'summary.json').write_text(json.dumps(result, indent=2))
    lines = ['| Type | Mode | Threads | bpp | Encode MPx/s | Decode MPx/s | Frontier |', '|---|---|---:|---:|---:|---:|---|']
    lines += [f"| {r['category']} | {r['arm']} | {r['threads']} | {r['bpp']:.4f} | {r['encode_mpx_s']:.2f} | {r['decode_mpx_s']:.2f} | {'yes' if r['frontier'] else ''} |" for r in aggregate]
    (output / 'table.md').write_text('\n'.join(lines) + '\n')
    html = Path(__file__).with_name('production_chart.html').read_text()
    (output / 'index.html').write_text(html.replace('/*DATA*/', json.dumps([r for r in aggregate if not r['control']]).replace('<', '\\u003c')))
    print(json.dumps({'observations': count, 'cells': len(rows), 'aggregate': aggregate}))


if __name__ == '__main__':
    main(*(Path(v).resolve() for v in sys.argv[1:]))
