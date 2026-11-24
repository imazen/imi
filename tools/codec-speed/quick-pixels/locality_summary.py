"""Verify retained PR33 data, preserving timings and their resource limitations."""
import gzip
import hashlib
import json
import math
from pathlib import Path
import statistics
import sys


def require(ok, message):
    if not ok:
        raise ValueError(message)


def sha(path):
    return hashlib.file_digest(path.open('rb'), 'sha256').hexdigest()


def same_projection(raw, normalized):
    # PR33 expands omitted defaults (cwd/env/streams/fields) in its packet.
    if isinstance(raw, dict):
        return all(k in normalized and same_projection(v, normalized[k]) for k, v in raw.items())
    if isinstance(raw, list):
        return len(raw) == len(normalized) and all(same_projection(a,b) for a,b in zip(raw, normalized))
    return raw == normalized


def resources(invocations):
    observations = [o for v in invocations for o in v.get('observations', [])]
    cpu = [o['cpu_load'] for o in observations if 'cpu_load' in o]
    busy = [o['heavy_process_count'] for o in observations if 'heavy_process_count' in o]
    return {'observations': len(observations),
            'all_cleanliness_certified': bool(observations) and all(o.get('cleanliness_certified') is True for o in observations),
            'max_cpu_load': max(cpu) if cpu else None,
            'max_heavy_process_count': max(busy) if busy else None}


def main(root, output):
    identities = json.loads((root / 'identities.json').read_text())
    packet = json.loads(gzip.decompress((root / 'paired/packet.json.gz').read_bytes()))
    plan = json.loads((root / 'plan.json').read_text())
    require(packet['complete'] and same_projection(plan, packet['plan']) and plan['blocks'] == 3 and plan['aa'], 'protocol')
    arms = [plan['baseline'], *plan['candidates']]
    cases = plan['cases']
    originals = {f['name']: f['sha256'] for f in identities.get('manifest', [])}
    if not originals:
        originals = {f['id']: f['sha256'] for f in identities['cases']}
    require(len(packet['invocations']) == len(cases) * (len(arms) + 1) * 3, 'observation count')
    cells, seen = {}, set()
    for v in packet['invocations']:
        case, arm, block = v['case'], v['arm'], v['block']
        require(0 <= case < len(cases) and 0 <= arm <= len(arms) and 0 <= block < 3, 'coordinates')
        key = case, arm, block
        require(key not in seen, 'duplicate observation')
        seen.add(key)
        require(v['status'] == 'completed' and v['exit_code'] == 0 and v['continuation_safe'], 'child status')
        require(len(v['values_ns']) == 1 and math.isfinite(v['values_ns'][0]) and v['values_ns'][0] > 0, 'duration')
        require(v['correctness'] == [originals[cases[case]['id']]], 'source pixels')
        for path, digest in v['artifacts'].items():
            local = root / 'paired' / Path(path).parent.name / Path(path).name
            require(sha(local) == digest, 'child evidence hash')
        cells.setdefault((case, arm), []).append(v['values_ns'][0])
    require(len(cells) == len(cases) * (len(arms)+1) and all(len(v) == 3 for v in cells.values()), 'cell coverage')
    require(all(s['complete_blocks'] == 3 and s['correctness_checked'] for s in packet['summaries']), 'checked blocks')
    rows, controls = [], []
    for i, case in enumerate(cases):
        baseline = max(cells[i, 0])
        controls.append({'case': case['id'], 'aa_worst_ratio': max(cells[i, len(arms)])/baseline})
        for j, arm in enumerate(arms):
            values = cells[i, j]
            rows.append({'case': case['id'], 'arm': arm['id'], 'median_ns': statistics.median(values),
                         'worst_ns': max(values), 'best_ns': min(values), 'worst_ratio': max(values)/baseline,
                         'resources': resources(v for v in packet['invocations'] if v['case'] == i and v['arm'] == j)})
    quality = resources(packet['invocations'])
    output.write_text(json.dumps({'observations': len(seen), 'rows': rows, 'controls': controls,
                                  'resources': quality,
                                  'qualification': 'Protocol completion and matching pixels do not certify idle-host codec performance.'}, indent=2))
    print(json.dumps({'observations': len(seen), 'cells': len(rows), 'aa': controls, 'resources': quality}, indent=2))


if __name__ == '__main__':
    main(*(Path(v).resolve() for v in sys.argv[1:]))
