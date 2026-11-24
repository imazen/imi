"""Replay the retained Quick photo regression with paired, endpoint-matched controls.

The original executables time native API calls after warmup; their CLI startup,
I/O and legacy checksums are outside the measured interval. New provenance uses
XXH3. The retained protocol's own hashes and files are never rewritten.
"""
import argparse
from datetime import datetime, timezone
import gzip
import json
import math
import os
from pathlib import Path
import statistics
import subprocess
import xxhash
from locality_summary import resources, same_projection, sha

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('action', choices=('prepare', 'run', 'summary'))
p.add_argument('root', type=Path)
p.add_argument('--original', type=Path)
p.add_argument('--summary-output', type=Path)
a = p.parse_args()
root = a.root.resolve()

def digest(path):
    with path.open('rb') as source:
        value = xxhash.xxh3_128()
        while block := source.read(1024 * 1024):
            value.update(block)
        return value.hexdigest()

def save(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')

if a.action == 'prepare':
    assert a.original is not None
    root.mkdir(exist_ok=True)
    assert not (root / 'REPLAY.json').exists()
    original = a.original.resolve()
    plan = json.loads((original / 'plan.json').read_text())
    arms = [plan['baseline'], *plan['candidates']]
    cases = [c for c in plan['cases'] if c['id'] in ('photo-1000', 'photo-1400', 'photo-1600')]
    assert len(cases) == 3
    frozen = {}
    jobs = []
    (root / 'plans').mkdir()
    for endpoint in ('decode-direct-file', 'decode-into-direct-file'):
        for workers in (1, 2, 4, 8):
            selected = []
            for implementation in ('old', 'new'):
                key = f'{implementation}-{endpoint}-{workers}'
                arm = next(x for x in arms if x['id'] == key)
                frozen[arm['program']] = digest(Path(arm['program']))
                selected.append(arm)
            name = f'{endpoint}-{workers}'
            current = dict(plan, baseline=selected[0], candidates=selected[1:], cases=cases,
                           blocks=24, aa=True, deadline_seconds=1200)
            path = root / 'plans' / (name + '.json')
            save(path, current)
            jobs.append(dict(name=name, plan=str(path), plan_xxh3_128=digest(path)))
    for case in cases:
        for value in case['inputs'].values():
            frozen[value] = digest(Path(value))
    runner = Path.home() / 'tmp/imij-real-photo-20261012/zenbench-pr33/bin/zenbench'
    frozen[str(runner)] = digest(runner)
    save(root / 'REPLAY.json', dict(original=str(original), runner=str(runner), jobs=jobs,
                                  frozen_xxh3_128=frozen, rounds=24, photos=3,
                                  scope='Historical replay, original binaries/files and measurement protocol; endpoint-matched A/A at every worker count. Not a current-V4 optimization verdict.'))
    print(json.dumps(dict(jobs=len(jobs), observations=len(jobs)*3*3*24)))
elif a.action == 'run':
    replay = json.loads((root / 'REPLAY.json').read_text())
    for path, expected in replay['frozen_xxh3_128'].items():
        assert digest(Path(path)) == expected, path
    (root / 'results').mkdir()
    (root / 'logs').mkdir()
    status = dict(stage='starting', controller_pid=os.getpid(), child_pid=None,
                  completed=[], required=len(replay['jobs']))
    for job in replay['jobs']:
        assert digest(Path(job['plan'])) == job['plan_xxh3_128']
        status.update(stage='running', current=job['name'], checked_utc=datetime.now(timezone.utc).isoformat())
        save(root / 'STATUS.json', status)
        command = [replay['runner'], 'pair', job['plan'], '--output', str(root / 'results' / job['name'])]
        with (root / 'logs' / (job['name'] + '.log')).open('x') as log:
            child = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT)
            status['child_pid'] = child.pid
            save(root / 'STATUS.json', status)
            rc = child.wait()
        status['child_pid'] = None
        assert rc == 0, (job['name'], rc)
        status['completed'].append(job['name'])
        save(root / 'STATUS.json', status)
        print(job['name'] + ' complete', flush=True)
    status.update(stage='complete', checked_utc=datetime.now(timezone.utc).isoformat())
    save(root / 'STATUS.json', status)
else:
    replay = json.loads((root / 'REPLAY.json').read_text())
    assert json.loads((root / 'STATUS.json').read_text())['stage'] == 'complete'
    for path, expected in replay['frozen_xxh3_128'].items():
        assert digest(Path(path)) == expected, path
    original = Path(replay['original'])
    identities = json.loads((original / 'identities.json').read_text())
    expected_pixels = {item['name']: item['sha256'] for item in identities['manifest']}
    original_plan = json.loads((original / 'plan.json').read_text())
    old_program = original_plan['baseline']['program']
    new_program = next(arm['program'] for arm in original_plan['candidates'] if arm['id'].startswith('new-'))
    # Verify the original protocol's retained seals; new provenance remains XXH3.
    for name, path in (('old', old_program), ('new', new_program), ('zenbench', replay['runner'])):
        assert sha(Path(path)) == identities[name], name
    rows = []
    for job in replay['jobs']:
        assert digest(Path(job['plan'])) == job['plan_xxh3_128']
        plan = json.loads(Path(job['plan']).read_text())
        packet = json.loads(gzip.decompress((root / 'results' / job['name'] / 'packet.json.gz').read_bytes()))
        assert packet['complete']
        assert same_projection(plan, packet['plan'])
        assert len(packet['invocations']) == 3*3*24
        for invocation in packet['invocations']:
            assert len(invocation['values_ns']) == 1
            assert math.isfinite(invocation['values_ns'][0]) and invocation['values_ns'][0] > 0
            assert invocation['correctness'] == [expected_pixels[plan['cases'][invocation['case']]['id']]]
            for path, expected in invocation['artifacts'].items():
                assert Path(path).resolve().is_relative_to(root / 'results' / job['name'])
                assert sha(Path(path)) == expected, path
        for case_id, case in enumerate(plan['cases']):
            values = {}
            checks = []
            for arm in (0, 1, 2):
                samples = [v for v in packet['invocations'] if v['case'] == case_id and v['arm'] == arm]
                assert len(samples) == 24 and len({v['block'] for v in samples}) == 24
                assert all(v['status'] == 'completed' and v['exit_code'] == 0 and v['continuation_safe'] for v in samples)
                assert len({tuple(v['correctness']) for v in samples}) == 1
                checks.append(samples[0]['correctness'])
                values[arm] = {v['block']: v['values_ns'][0] for v in samples}
            assert checks[0] == checks[1] == checks[2], case['id']
            ratios = [values[1][i] / values[0][i] for i in range(24)]
            aa = [values[2][i] / values[0][i] for i in range(24)]
            rows.append(dict(job=job['name'], photo=case['id'],
                             baseline_ns=statistics.median(values[0].values()),
                             candidate_ns=statistics.median(values[1].values()),
                             paired_ratio_median=statistics.median(ratios), aa_ratio_median=statistics.median(aa),
                             baseline_max_ns=max(values[0].values()), candidate_max_ns=max(values[1].values()),
                             resources=resources(v for v in packet['invocations'] if v['case'] == case_id),
                             baseline_values=values[0], candidate_values=values[1], aa_values=values[2]))
    output = a.summary_output or root / 'SUMMARY.json'
    assert not output.exists()
    save(output, rows)
    for row in rows:
        print(json.dumps({k:v for k,v in row.items() if not k.endswith('_values')}))
