"""Validate complete PR33 observations and report slowest-of-three throughput."""
import gzip
import hashlib
import json
import math
from pathlib import Path
import sys


def require(ok, message):
    if not ok:
        raise ValueError(message)


def main(root, output):
    ids = json.loads((root / 'identities.json').read_text())
    manifest = [f for f in ids['manifest'] if f['sample'] == 'u8' and f['channels'] in ('rgb', 'rgba')]
    arms = ['%s-%s-%s' % (c, r, t) for c, t, r in ids['arms']]
    prepared = json.loads((root / 'prepared.json').read_text())
    for p in prepared:
        path = root / f"{p['image']}-{p['arm']}.bin"
        require(hashlib.sha256(path.read_bytes()).hexdigest() == p['sha256'], 'encoded hash')
    times, aa = {}, []
    count = 0
    for action in ('encode', 'decode'):
        packet = json.loads(gzip.decompress((root / action / 'packet.json.gz').read_bytes()))
        require(packet['complete'] and packet['plan']['blocks'] == 3 and packet['plan']['aa'], 'protocol')
        require(len(packet['invocations']) == len(manifest) * (len(arms)+1) * 3, 'count')
        seen, cells = set(), {}
        for v in packet['invocations']:
            key = v['case'], v['arm'], v['block']
            require(key not in seen, 'duplicate')
            seen.add(key)
            require(v['status'] == 'completed' and v['exit_code'] == 0 and v['continuation_safe'], 'child status')
            require(len(v['values_ns']) == 1 and math.isfinite(v['values_ns'][0]) and v['values_ns'][0] > 0, 'time')
            require(v['correctness'] == [manifest[v['case']]['sha256']], 'original pixel digest')
            for path, sha in v['artifacts'].items():
                local = root / action / Path(path).parent.name / Path(path).name
                require(hashlib.sha256(local.read_bytes()).hexdigest() == sha, 'child artifact hash')
            stdout = (root / action / Path(v['output']).name / 'stdout.txt').read_text()
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
    for f in manifest:
        for arm in arms:
            p = next(p for p in prepared if p['image'] == f['name'] and p['arm'] == arm)
            row = {'image': f['name'], 'category': f['category'], 'arm': arm, 'bytes': p['bytes'], 'pixels': f['width']*f['height']}
            for action in ('encode', 'decode'):
                row[action+'_ns'] = times[f['name'], arm, action]
            rows.append(row)
    aggregate = []
    for category in dict.fromkeys(f['category'] for f in manifest):
        for arm in arms:
            cells = [r for r in rows if r['category'] == category and r['arm'] == arm]
            pixels = sum(c['pixels'] for c in cells)
            aggregate.append({'category': category, 'arm': arm, 'bpp': 8*sum(c['bytes'] for c in cells)/pixels,
                              **{a+'_mpx_s': pixels*1000/sum(c[a+'_ns'] for c in cells) for a in ('encode', 'decode')}})
    output.write_text(json.dumps({'observations': count, 'rows': rows, 'aggregate': aggregate, 'aa': aa}, indent=2))
    print(json.dumps(aggregate, indent=2))


if __name__ == '__main__':
    main(*(Path(v).resolve() for v in sys.argv[1:]))
