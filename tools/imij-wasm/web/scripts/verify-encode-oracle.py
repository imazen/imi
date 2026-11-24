#!/usr/bin/env python3
"""Independently decode every retained encode cell and compare its source hash."""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('oracle', type=Path)
p.add_argument('fixtures', type=Path)
p.add_argument('measurements', type=Path)
p.add_argument('output', type=Path)
a = p.parse_args()
a.output.mkdir()
sha = lambda data: hashlib.sha256(data).hexdigest()
summary = json.loads((a.measurements / 'summary.json').read_text())
if not summary['complete'] or summary['cells'] != summary['planned']:
    raise ValueError('incomplete measurement')
sources = {f['raw']: f for f in json.loads((a.fixtures / 'manifest.json').read_text())}
results = []
for i, row in enumerate(summary['rows']):
    encoded = a.measurements / f'cell-{i}.imi'
    if sha(encoded.read_bytes()) != row['encodedSha']:
        raise ValueError('encoded hash mismatch')
    raw = a.output / f'cell-{i}.raw'
    command = [str(a.oracle), 'decode', str(encoded), str(raw)]
    process = subprocess.run(command, capture_output=True)
    (a.output / f'cell-{i}.stdout').write_bytes(process.stdout)
    (a.output / f'cell-{i}.stderr').write_bytes(process.stderr)
    digest = sha(raw.read_bytes()) if raw.exists() else None
    expected = sources[row['image']]['sha256']
    result = dict(cell=i, command=command, rc=process.returncode, sha256=digest,
                  expected=expected, passed=process.returncode == 0 and digest == expected)
    results.append(result)
    (a.output / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
print(json.dumps(dict(passed=sum(r['passed'] for r in results), total=len(results),
                      oracleSha256=sha(a.oracle.read_bytes()))))
raise SystemExit(0 if all(r['passed'] for r in results) else 1)
