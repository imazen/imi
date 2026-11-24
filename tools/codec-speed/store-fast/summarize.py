"""Reconcile complete native PR33 observations and byte-exact browser cells."""
import gzip
import hashlib
import json
import math
import pathlib
import sys


def require(value, message):
    if not value:
        raise ValueError(message)


def main(root, output):
    native = root / 'results-v2'
    prepared = json.loads((native / 'prepared.json').read_text())
    identities = json.loads((native / 'identities.json').read_text())
    manifest = identities['manifest']
    browser = [json.loads(line) for line in (root / 'browser-results-v2/rows.jsonl').read_text().splitlines()]
    require(len(prepared) == len(browser) == 44, 'complete cells')
    timings, noise = {}, []
    observations = 0
    for group, modes, images in [('all', [0, 1, 2, 3, 5], manifest), ('green', [0, 4], [f for f in manifest if (f['channels'], f['sample']) == ('rgb', 'u8')])]:
        for action in ['encode', 'decode']:
            p = json.loads(gzip.decompress((native / f'{group}-{action}/packet.json.gz').read_bytes()))
            require(p['complete'] and p['plan']['blocks'] == 3 and p['plan']['aa'], 'protocol')
            require(len(p['invocations']) == len(images) * (len(modes) + 1) * 3, 'observations')
            seen, cells = set(), {}
            for o in p['invocations']:
                require(o['status'] == 'completed' and o['exit_code'] == 0 and o['continuation_safe'], 'failed child')
                key = o['case'], o['arm'], o['block']
                require(key not in seen, 'duplicate observation')
                seen.add(key)
                require(len(o['values_ns']) == 1 and math.isfinite(o['values_ns'][0]) and o['values_ns'][0] > 0, 'time')
                cells.setdefault(key[:2], []).append(o['values_ns'][0])
                for path, digest in o['artifacts'].items():
                    # Recorded paths are absolute on the capture host; keep the part
                    # below the results directory.
                    parts = pathlib.Path(path).parts
                    local = native / pathlib.Path(*parts[parts.index('results-v2') + 1:])
                    require(hashlib.sha256(local.read_bytes()).hexdigest() == digest, 'process artifact hash')
            require(all(len(v) == 3 for v in cells.values()), 'three repeats')
            require(all(s['complete_blocks'] == 3 and s['correctness_checked'] for s in p['summaries']), 'checks')
            for i, f in enumerate(images):
                baseline = max(cells[i, 0])
                noise.append({'group': group, 'action': action, 'image': f['name'], 'aa_worst_ratio': max(cells[i, len(modes)]) / baseline})
                for j, mode in enumerate(modes):
                    if group == 'green' and mode == 0:
                        continue
                    timings[f['name'], mode, action] = max(cells[i, j])
            observations += len(p['invocations'])
    rows = []
    for f in manifest:
        for mode in range(6):
            found = [p for p in prepared if p['image'] == f['name'] and p['mode'] == mode]
            if not found:
                require(mode == 4 and (f['channels'], f['sample']) != ('rgb', 'u8'), 'missing candidate')
                continue
            require(len(found) == 1, 'duplicate preparation')
            p = found[0]
            data = native / f"{f['name']}-{mode}.spx"
            require(hashlib.sha256(data.read_bytes()).hexdigest() == p['sha256'], 'prepared bytes')
            b = [r for r in browser if (r['image'], r['mode']) == (f['name'], mode)]
            require(len(b) == 1 and len(b[0]['results']) == 3, 'browser repetitions')
            r = {'image': f['name'], 'category': f['category'], 'mode': mode, 'pixels': f['width'] * f['height'], 'bytes': p['bytes']}
            for action in ['encode', 'decode']:
                r['native_' + action + '_ns'] = timings[f['name'], mode, action]
                values = [v[action + '_ns'] for v in b[0]['results']]
                require(all(math.isfinite(v) and v > 0 for v in values), 'browser time')
                r['browser_' + action + '_ns'] = max(values)
            rows.append(r)
    aggregate = []
    for category in dict.fromkeys(r['category'] for r in rows):
        for mode in range(6):
            cells = [r for r in rows if r['category'] == category and r['mode'] == mode]
            if not cells:
                continue
            pixels = sum(r['pixels'] for r in cells)
            r = {'category': category, 'mode': mode, 'bpp': 8 * sum(r['bytes'] for r in cells) / pixels}
            for host in ['native', 'browser']:
                for action in ['encode', 'decode']:
                    r[host + '_' + action + '_mpx_s'] = pixels * 1000 / sum(c[host + '_' + action + '_ns'] for c in cells)
            aggregate.append(r)
    output.write_text(json.dumps({'observations': observations, 'browser_observations': len(browser) * 3, 'rows': rows, 'aggregate': aggregate, 'aa': noise}, indent=2))
    print(json.dumps(aggregate, indent=2))

if __name__ == '__main__':
    main(*(pathlib.Path(a).resolve() for a in sys.argv[1:]))
