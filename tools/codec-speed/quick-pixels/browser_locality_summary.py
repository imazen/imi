"""Verify paired browser packets and report absolute worst-of-three cells."""
import json
import math
from pathlib import Path
import sys


def main(root, manifest, output):
    images = json.loads(manifest.read_text())
    source = {f['name']: f for f in images}
    expected = {(f['name'] + '/' + mode, variant, threads)
                for f in images for mode in ('quick', '0', '7.5')
                for variant in ('simd', 'relaxed', 'threads', 'relaxed-threads')
                for threads in ((1, 2, 4, 8) if 'threads' in variant else (1,))}
    cells, versions = {}, set()
    for arm in ('baseline', 'candidate'):
        for block in range(3):
            lines = (root / f'{arm}-{block}.log').read_text().splitlines()
            packets = [json.loads(s.removeprefix('observations='))
                       for s in lines if s.startswith('observations=')]
            if len(packets) != 1:
                raise ValueError('missing or duplicate browser packet')
            packet = packets[0]
            versions.add(packet['version'])
            seen = set()
            for row in packet['rows']:
                key = row['name'], row['variant'], row['threads']
                if key not in expected or key in seen:
                    raise ValueError('unexpected or duplicate cell')
                seen.add(key)
                f = source[row['name'].rsplit('/', 1)[0]]
                if row['hash'] != f['sha256'] or (row['width'], row['height']) != (f['width'], f['height']):
                    raise ValueError('decoded source identity')
                if not all(math.isfinite(row[k]) and row[k] > 0 for k in ('workerMs', 'endToEndMs')):
                    raise ValueError('invalid timing')
                cells.setdefault((arm, *key), []).append(row)
            if seen != expected:
                raise ValueError('incomplete variant/thread product')
    if len(versions) != 1 or any(len(v) != 3 for v in cells.values()):
        raise ValueError('three observations of one browser required')
    rows = []
    for name, variant, threads in sorted(expected):
        row = {'name': name, 'variant': variant, 'threads': threads}
        f = source[name.rsplit('/', 1)[0]]
        for arm in ('baseline', 'candidate'):
            observations = cells[arm, name, variant, threads]
            for timing in ('workerMs', 'endToEndMs'):
                value = max(r[timing] for r in observations)
                row[f'{arm}_{timing}'] = value
                row[f'{arm}_{timing}_mpx_s'] = f['width'] * f['height'] / value / 1000
        row['worker_time_ratio'] = row['candidate_workerMs'] / row['baseline_workerMs']
        rows.append(row)
    output.write_text(json.dumps({'browser': next(iter(versions)),
                                 'observations': sum(map(len, cells.values())),
                                 'protocol': 'alternating baseline/candidate; three fresh processes; worst observation',
                                 'rows': rows}, indent=2))


if __name__ == '__main__':
    main(*(Path(v).resolve() for v in sys.argv[1:]))
