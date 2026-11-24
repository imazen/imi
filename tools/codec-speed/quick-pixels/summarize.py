"""Reconcile source-qualified native and exact browser observations."""
import gzip
import hashlib
import json
import math
from pathlib import Path
import sys


def require(ok, msg):
    if not ok:
        raise ValueError(msg)


def main(root, output):
    native = root / 'measurement/results'
    p = json.loads((native / 'identities.json').read_text())
    manifest = p['manifest']
    prepared = json.loads((native / 'prepared.json').read_text())
    browser = [json.loads(s) for s in (root / 'measurement/browser-results/rows.jsonl').read_text().splitlines()]
    require(len(prepared) == len(browser) == 13, 'real cells')
    times, aa = {}, []
    total = 0
    for group in ('all', 'rgb'):
        names = [f['name'] for f in manifest if group == 'all' or (f['sample'] == 'u8' and f['channels'] in ('rgb', 'rgba'))]
        for action in ('encode', 'decode'):
            packet = json.loads(gzip.decompress((native / f'{group}-{action}/packet.json.gz').read_bytes()))
            require(packet['complete'] and packet['plan']['blocks'] == 3 and packet['plan']['aa'], 'protocol')
            require(len(packet['invocations']) == len(names) * 3 * 3, 'count')
            seen, cells = set(), {}
            for v in packet['invocations']:
                key = v['case'], v['arm'], v['block']
                require(key not in seen, 'duplicate')
                seen.add(key)
                require(v['status'] == 'completed' and v['exit_code'] == 0 and v['continuation_safe'], 'process')
                require(len(v['values_ns']) == 1 and math.isfinite(v['values_ns'][0]) and v['values_ns'][0] > 0, 'time')
                cells.setdefault(key[:2], []).append(v['values_ns'][0])
                for path, digest in v['artifacts'].items():
                    # Recorded paths are absolute on the capture host; keep the part
                    # below the results directory.
                    parts = Path(path).parts
                    last = len(parts) - 1 - parts[::-1].index('results')
                    path = Path(*parts[last + 1:])
                    require(hashlib.sha256((native / path).read_bytes()).hexdigest() == digest, 'artifact hash')
            require(all(len(v) == 3 for v in cells.values()), 'repetitions')
            require(all(s['complete_blocks'] == 3 and s['correctness_checked'] for s in packet['summaries']), 'checks')
            for i, name in enumerate(names):
                aa.append({'group': group, 'action': action, 'image': name, 'ratio': max(cells[i, 2]) / max(cells[i, 0])})
                for j, arm in enumerate(['old-packets' if group == 'all' else 'qoi', 'wide']):
                    if group == 'rgb' and arm == 'wide':
                        continue
                    times[name, arm, action] = max(cells[i, j])
            total += len(packet['invocations'])
    rows = []
    for f in manifest:
        for mode in ['wide'] + (['qoi'] if f['sample'] == 'u8' and f['channels'] in ('rgb', 'rgba') else []):
            matches = [v for v in prepared if v['image'] == f['name'] and v['mode'] == mode]
            b = [v for v in browser if v['image'] == f['name'] and v['mode'] == mode]
            require(len(matches) == len(b) == 1 and len(b[0]['results']) == 3, 'coverage')
            encoded = native / f"{f['name']}-{mode}.bin"
            require(hashlib.sha256(encoded.read_bytes()).hexdigest() == matches[0]['sha256'], 'encoded hash')
            row = {'image': f['name'], 'category': f['category'], 'mode': mode, 'bytes': matches[0]['bytes'], 'pixels': f['width'] * f['height']}
            for action in ('encode', 'decode'):
                values = [v[action + '_ns'] for v in b[0]['results']]
                require(all(math.isfinite(v) and v > 0 for v in values), 'browser time')
                row['browser_' + action + '_ns'] = max(values)
                row['native_' + action + '_ns'] = times[f['name'], mode, action]
                row['old_packets_' + action + '_ns'] = times[f['name'], 'old-packets', action]
            rows.append(row)
    aggregate = []
    for category in dict.fromkeys(r['category'] for r in rows):
        for mode in ['qoi', 'wide']:
            cells = [r for r in rows if r['category'] == category and r['mode'] == mode]
            if not cells:
                continue
            pixels = sum(c['pixels'] for c in cells)
            row = {'category': category, 'mode': mode, 'bpp': 8 * sum(c['bytes'] for c in cells) / pixels}
            for host in ('native', 'browser', 'old_packets'):
                for action in ('encode', 'decode'):
                    row[host + '_' + action + '_mpx_s'] = pixels * 1000 / sum(c[host + '_' + action + '_ns'] for c in cells)
            aggregate.append(row)
    output.write_text(json.dumps({'native_observations': total, 'browser_observations': len(browser)*3, 'rows': rows, 'aggregate': aggregate, 'aa': aa}, indent=2))
    print(json.dumps(aggregate, indent=2))


if __name__ == '__main__':
    main(*(Path(v).resolve() for v in sys.argv[1:]))
