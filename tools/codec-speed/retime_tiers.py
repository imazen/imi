"""Pair policies together using retained fixtures and exact per-file pixel oracles."""
import gzip
import json
import math
import pathlib
import sys
from fast_lossless import execute, require, sha
from lossy_tiers import IMAGES, QUALITIES, POLICIES, PIXELS


def check(output):
    rows = [json.loads(line[len('check='):]) for line in output.splitlines() if line.startswith('check=')]
    require(len(rows) == 1, 'missing/duplicate pixel check')
    return rows[0]


def run(prior, zenbench, root):
    root.mkdir()
    previous = json.loads(gzip.decompress((prior / 'decode/packet.json.gz').read_bytes()))
    require(previous['complete'], 'prior correctness run incomplete')
    reference = {}
    for obs in previous['invocations']:
        require(obs['status'] == 'completed' and obs['exit_code'] == 0, 'invalid reference')
        for filename, digest in obs['artifacts'].items():
            require(sha(pathlib.Path(filename)) == digest, 'reference artifact changed')
        case = previous['plan']['cases'][obs['case']]['id']
        value = check((pathlib.Path(obs['output']) / 'stdout.txt').read_text())
        require(value['bytes'] == PIXELS * 3, 'reference dimensions')
        require(reference.setdefault(case, value) == value, 'reference thread disagreement')
    identities = json.loads((prior / 'identities.json').read_text())
    require(all(sha(pathlib.Path(p)) == h for p, h in identities.items()), 'source/program changed')
    preparations = json.loads((prior / 'preparations.json').read_text())
    for key, value in preparations.items():
        require(sha(prior / 'files' / f'{key}.imi') == value['encoded_sha256'], 'fixture changed')
    probe = previous['plan']['baseline']['program']
    cases, arms = [], []
    for image in IMAGES:
        for quality in QUALITIES:
            inputs = {'raw': str(prior / 'files' / f'{image}.rgb')}
            inputs.update({p: str(prior / 'files' / f'{image}-q{quality}-{p}.imi') for p in POLICIES})
            cases.append({'id': f'{image}-q{quality}', 'inputs': inputs, 'fields': {'quality': str(quality)}})
    for policy in POLICIES:
        for threads in [1, 2, 4, 8]:
            arms.append({'id': f'{policy}-{threads}', 'program': probe, 'cwd': str(root), 'env': {},
                         'args': ['decode', 'imij-' + policy, '${quality}', str(threads), '${raw}', '${' + policy + '}'],
                         'timeout_seconds': 30, 'revision': sha(pathlib.Path(probe))})
    plan = {'schema': 1, 'baseline': arms[0], 'candidates': arms[1:], 'cases': cases,
            'metric': {'extract': {'kind': 'prefix', 'prefix': 'elapsed_ms='}, 'stream': 'stdout', 'unit': 'ms', 'expected': 1, 'divide_by': 1},
            # Different policies can reconstruct different rasters. Full per-file
            # pixel digests are checked below against all previous thread routes.
            'check': {'kind': 'json', 'prefix': 'check=', 'pointers': ['/bytes'], 'stream': 'stdout'},
            'warmup': 'Three requested-route warmups plus serial pixel check, ten allocating timed decodes.',
            'blocks': 3, 'aa': True, 'seed': 731, 'settle_ms': 0, 'deadline_seconds': 600,
            'env': {}, 'clean_env': False, 'record_env': []}
    (root / 'plan.json').write_text(json.dumps(plan, indent=2))
    (root / 'reference.json').write_text(json.dumps(reference, indent=2))
    execute([str(zenbench), 'pair', str(root / 'plan.json'), '--output', str(root / 'paired')], root / 'run.log')
    packet = json.loads(gzip.decompress((root / 'paired/packet.json.gz').read_bytes()))
    require(packet['complete'] and packet['plan'] == plan, 'incomplete/changed plan')
    require(len(packet['invocations']) == len(cases) * (len(arms) + 1) * 3, 'missing observations')
    seen, cells = set(), {}
    for obs in packet['invocations']:
        require(obs['status'] == 'completed' and obs['exit_code'] == 0 and obs['continuation_safe'], 'failed process')
        key = (obs['case'], obs['arm'], obs['block'])
        require(key not in seen and 0 <= key[0] < len(cases) and 0 <= key[1] <= len(arms) and 0 <= key[2] < 3, 'invalid observation')
        seen.add(key)
        arm = arms[key[1]] if key[1] < len(arms) else arms[0]
        policy, threads = arm['id'].split('-')
        case = cases[key[0]]['id']
        for filename, digest in obs['artifacts'].items():
            require(sha(pathlib.Path(filename)) == digest, 'changed artifact')
        actual = check((pathlib.Path(obs['output']) / 'stdout.txt').read_text())
        require(actual == reference[case + '-' + policy], 'pixels differ from retained serial/threaded oracle')
        values = obs['values_ns']
        require(len(values) == 1 and math.isfinite(values[0]) and values[0] > 0, 'invalid time')
        name = arm['id'] if key[1] < len(arms) else '__aa'
        cells.setdefault((case, name), []).append(values[0] / 1e6)
    require(all(len(v) == 3 for v in cells.values()), 'missing repeats')
    require(all(s['complete_blocks'] == 3 and s['correctness_checked'] for s in packet['summaries']), 'invalid packet summary')
    summary = json.loads((prior / 'summary.json').read_text())
    for row in summary['rows']:
        for threads in [1, 2, 4, 8]:
            times = [max(cells[(f'{i}-q{row["quality"]}', f'{row["policy"]}-{threads}')]) for i in IMAGES]
            row[f'decode_{threads}t_ms_by_image'] = times
            row[f'decode_{threads}t_mpx_s'] = 3 * PIXELS / (1000 * sum(times))
    summary['aa']['decode'] = [s for s in packet['summaries'] if s['arm'] == '__aa']
    summary['observations']['decode'] = len(packet['invocations'])
    summary['decode_packet'] = str(root / 'paired/packet.json.gz')
    summary['encode_packet'] = str(prior / 'encode/packet.json.gz')
    summary['all_per_file_pixel_oracles_match'] = True
    require(all(sha(pathlib.Path(p)) == h for p, h in identities.items()), 'source/program changed after timing')
    require(all(sha(prior / 'files' / f'{k}.imi') == v['encoded_sha256'] for k, v in preparations.items()), 'fixture changed after timing')
    (root / 'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))


if __name__ == '__main__':
    require(len(sys.argv) == 4, 'retime_tiers.py PRIOR-MEASUREMENT ZENBENCH NEW-OUTPUT')
    run(*(pathlib.Path(v).resolve() for v in sys.argv[1:]))
