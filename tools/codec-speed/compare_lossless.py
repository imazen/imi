"""Paired encoder change check against retained lossless or lossy-tier files."""
import argparse
import gzip
import json
import math
import pathlib
from fast_lossless import IMAGES, PIXELS, execute, metric, require, sha


def run(baseline, candidate, prior, zenbench, root, mode="lossless", resume=None):
    require(mode in ["lossless", "lossy", "reader", "feed"], "unknown comparison domain")
    reader = mode in ("reader", "feed")
    require(not reader or resume is None, "reader resume not implemented")
    folder = prior / ("results" if mode != "lossy" else "files")
    settings = [("imij-lossless", e, "e" + e) for e in ["0", "2.5", "7.5"]] if mode != "lossy" else [("imij-" + p, q, "q" + q + "-" + p) for q in ["20", "50", "80"] for p in ["default", "compact", "balanced", "fast", "huffman"]]
    root.mkdir()
    (root / 'files').mkdir()
    identities = {str(p): sha(p) for p in [baseline, candidate, zenbench]}
    cases, encode_ms = [], 0.0
    for image in IMAGES:
        raw = folder / f'{image}.rgb'
        require(raw.stat().st_size == PIXELS * 3, 'wrong input size')
        identities[str(raw)] = sha(raw)
        for codec, effort, label in settings:
            name = f'{image}-imij-lossless-{effort}.bin' if mode != 'lossy' else f'{image}-{label}.imi'
            original = folder / name
            identities[str(original)] = sha(original)
            created = root / 'files' / name
            if reader:
                created = original
            elif resume is None:
                log = execute([str(candidate), 'prepare', codec, effort, '1', str(raw), str(created)], root / f'prepare-{name}.log')
            else:
                created = resume / 'files' / name
                log_path = resume / f'prepare-{name}.log'
                require(log_path.with_suffix('.rc').read_text().strip() == '0', 'failed original preparation')
                log = log_path.read_text()
            if not reader:
                encode_ms += metric(log, 'encode_total_ms')
                require(created.read_bytes() == original.read_bytes(), 'encoded bytes changed')
            cases.append({'id': f'{image}-{label}', 'inputs': {'raw': str(raw), 'encoded': str(original)}, 'fields': {'effort': effort, 'codec': codec}})
    arms = [{'id': name, 'program': str(program), 'cwd': str(resume or root), 'env': {},
             'args': ['feed' if mode == 'feed' and name == 'after' else 'reader' if reader else 'encode', '${codec}', '${effort}', '1', '${raw}', '${encoded}'],
             'timeout_seconds': 30, 'revision': identities[str(program)]}
            for name, program in [('before', baseline), ('after', candidate)]]
    plan = {'schema': 1, 'baseline': arms[0], 'candidates': arms[1:], 'cases': cases,
            'metric': {'extract': {'kind': 'prefix', 'prefix': 'elapsed_ms='}, 'stream': 'stdout', 'unit': 'ms', 'expected': 1, 'divide_by': 1},
            'check': {'kind': 'json', 'prefix': 'check=', 'pointers': ['/sha256', '/bytes'] if reader else ['/source_sha256', '/bytes', '/matches_prepared'], 'stream': 'stdout'},
            'warmup': ('Three reader warmups (first validates exact pixels), ten row-callback decodes divided by ten. Timed callback does not allocate/accumulate a raster; validation and raw rasters remain alive outside it.' if reader else 'One encode warmup and one observation; both byte-equal the retained original.'),
            'blocks': 3, 'aa': True, 'seed': 731, 'settle_ms': 0, 'deadline_seconds': 180,
            'env': {}, 'clean_env': False, 'record_env': []}
    if mode == 'feed':
        plan['warmup'] += ' Candidate uses a 64 KiB caller-feed queue, request admission and one copied typed provisional row; baseline is blocking reader. Both include final validation.'
    (root / 'plan.json').write_text(json.dumps(plan, indent=2))
    (root / 'identities.json').write_text(json.dumps(identities, indent=2))
    argv = [str(zenbench), 'pair', str(root / 'plan.json'), '--wait', '180', '--output', str(root / 'paired')]
    earlier = None
    if resume is not None:
        require(json.loads((resume / 'plan.json').read_text()) == plan, 'resume plan changed')
        require(json.loads((resume / 'identities.json').read_text()) == identities, 'resume identities changed')
        earlier = json.loads(gzip.decompress((resume / 'paired/packet.json.gz').read_bytes()))
        require(earlier['plan'] == plan, 'earlier packet plan changed')
        argv += ['--resume', str(resume / 'paired')]
    execute(argv, root / 'run.log')
    packet = json.loads(gzip.decompress((root / 'paired/packet.json.gz').read_bytes()))
    require(packet['complete'] and packet['plan'] == plan, 'incomplete/changed plan')
    require(len(packet['invocations']) == len(cases) * 9, 'incomplete observations')
    seen, cells, checks = set(), {}, {}
    for obs in packet['invocations']:
        require(obs['status'] == 'completed' and obs['exit_code'] == 0 and obs['continuation_safe'], 'failed process')
        key = (obs['case'], obs['arm'], obs['block'])
        require(key not in seen and 0 <= key[0] < len(cases) and 0 <= key[1] < 3 and 0 <= key[2] < 3, 'invalid observation')
        seen.add(key)
        values = obs['values_ns']
        require(len(values) == 1 and math.isfinite(values[0]) and values[0] > 0, 'invalid time')
        for filename, digest in obs['artifacts'].items():
            require(sha(pathlib.Path(filename)) == digest, 'artifact changed')
        name = ['before', 'after', '__aa'][key[1]]
        cells.setdefault((cases[key[0]]['id'], name), []).append(values[0] / 1e6)
        checks.setdefault((key[0], key[2]), []).append(obs['correctness'])
        if not reader:
            encode_ms += metric((pathlib.Path(obs['output']) / 'stdout.txt').read_text(), 'encode_total_ms')
    require(all(len(v) == 3 for v in cells.values()), 'missing repeat')
    require(all(all(v == group[0] for v in group) for group in checks.values()), 'byte/source mismatch')
    require(all(s['complete_blocks'] == 3 and s['correctness_checked'] for s in packet['summaries']), 'invalid summary')
    earlier_only = 0
    if earlier is not None:
        reused_outputs = {obs['output'] for obs in packet['invocations']}
        for obs in earlier['invocations']:
            if obs['output'] not in reused_outputs and obs['status'] == 'completed':
                require(obs['exit_code'] == 0, 'failed earlier process')
                for filename, digest in obs['artifacts'].items():
                    require(sha(pathlib.Path(filename)) == digest, 'earlier artifact changed')
                encode_ms += metric((pathlib.Path(obs['output']) / 'stdout.txt').read_text(), 'encode_total_ms')
                earlier_only += 1
    require(encode_ms < 60000, 'additional encode budget exceeded')
    require(all(sha(pathlib.Path(p)) == h for p, h in identities.items()), 'source/program changed')
    rows = []
    for codec, effort, label in settings:
        row = {'codec': codec, 'effort' if mode != 'lossy' else 'quality': float(effort)}
        for name in ['before', 'after']:
            times = [max(cells[(f'{i}-{label}', name)]) for i in IMAGES]
            row[name + '_ms_by_image'] = times
            row[name + '_mpx_s'] = 3 * PIXELS / (1000 * sum(times))
        row['bytes_by_image'] = [(folder / (f'{i}-imij-lossless-{effort}.bin' if mode != 'lossy' else f'{i}-{label}.imi')).stat().st_size for i in IMAGES]
        row['bits_per_pixel'] = 8 * sum(row['bytes_by_image']) / (3 * PIXELS)
        rows.append(row)
    summary = {'rows': rows, 'all_encoded_bytes_unchanged': None if reader else True, 'retained_encoded_inputs_unchanged': True, 'observations': len(cases) * 9, 'mode': mode,
               'all_encode_seconds': encode_ms / 1000, 'earlier_only_observations': earlier_only,
               'resumed_from': str(resume) if resume else None, 'paired_summaries': packet['summaries']}
    (root / 'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['baseline', 'candidate', 'prior', 'zenbench', 'root']:
        parser.add_argument(name, type=lambda value: pathlib.Path(value).resolve())
    parser.add_argument('mode', nargs='?', default='lossless', choices=['lossless', 'lossy', 'reader', 'feed'])
    parser.add_argument('--resume', type=lambda value: pathlib.Path(value).resolve())
    run(**vars(parser.parse_args()))
