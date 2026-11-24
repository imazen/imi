"""Compare source-qualified layout/part choices on one frozen browsing trace.

No encoder/quality change. Every scenario must match the original trace's pixel
hashes/domain, not merely other scenarios within the same candidate layout.
"""
import argparse
import hashlib
import itertools
import json
from pathlib import Path
import subprocess
import sys


def run(command, prefix):
    prefix.with_suffix('.command.json').write_text(json.dumps(command) + '\n')
    with prefix.with_suffix('.log').open('x') as log:
        result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=False)
    prefix.with_suffix('.rc').write_text(str(result.returncode) + '\n')
    if result.returncode:
        raise RuntimeError(f'{prefix.name} failed; files retained')


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def original_entry_keys(domains, original_trace, candidate_trace):
    """Inverse only the explicit key renumbering; retain exact pixel/hash checks."""
    def rows(path):
        return [line.split('\t') for line in path.read_text().splitlines()
                if line and not line.startswith('#')]
    old, new = rows(original_trace), rows(candidate_trace)
    if len(old) != len(new):
        raise ValueError('layout trace length')
    inverse = {}
    for a, b in zip(old, new):
        if a[0] != b[0]:
            raise ValueError('layout trace labels')
        left = [] if a[4] == '-' else a[4].split(',')
        right = [] if b[4] == '-' else b[4].split(',')
        if len(left) != len(right) or len(set(right)) != len(right):
            raise ValueError('layout key domain')
        inverse.update({(a[0], k): v for k, v in zip(right, left)})
    return [(label, inverse[(label, key)], size, pixel_hash)
            for label, key, size, pixel_hash in domains]


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('replay_binary', type=Path)
    p.add_argument('layout_binary', type=Path)
    p.add_argument('trace', type=Path)
    p.add_argument('output', type=Path)
    p.add_argument('--rtts', type=float, nargs='+', default=[0, 160])
    p.add_argument('--caches', type=int, nargs='+', default=[0, 65536, 33554432])
    p.add_argument('--concurrencies', type=int, nargs='+', default=[1, 8])
    p.add_argument('--part-bytes', type=int, nargs='+', default=[0, 16384, 65536])
    p.add_argument('--organizations', choices=['separate', 'packed', 'flat'], nargs='+', default=['separate', 'packed'])
    a = p.parse_args()
    if any(n < 0 for n in a.part_bytes) or len(set(a.part_bytes)) != len(a.part_bytes):
        p.error('distinct nonnegative part sizes required')
    a.output.mkdir(parents=True, exist_ok=False)
    matrix = Path(__file__).with_name('run-access-model.py')
    replay, layout, trace = (str(p.resolve()) for p in (a.replay_binary, a.layout_binary, a.trace))
    source_paths = set()
    for line in a.trace.read_text().splitlines():
        if line and not line.startswith('#'):
            path = Path(line.split('\t')[2])
            source_paths.add(path)
            if path.name.endswith('.parts.tsv'):
                for row in path.read_text().splitlines()[1:]:
                    source_paths.add(path.parent / row.split('\t')[0])
    source_hashes = {str(p.resolve()): digest(p) for p in sorted(source_paths)}
    identity = {'source_sha256': source_hashes, 'replay_sha256': digest(a.replay_binary), 'layout_sha256': digest(a.layout_binary),
                'trace_sha256': digest(a.trace), 'part_bytes': a.part_bytes,
                'cache_scope': 'shared', 'timing': 'simulated network only; no CPU timing prediction'}
    (a.output / 'identity.json').write_text(json.dumps(identity, indent=2) + '\n')
    policy_args = ['--rtts', *map(str, a.rtts), '--caches', *map(str, a.caches), '--concurrencies', *map(str, a.concurrencies)]
    reference_dir = a.output / 'original'
    run([sys.executable, str(matrix), replay, trace, str(reference_dir), '--cache-scope', 'shared', *policy_args], a.output / 'original-run')
    reference = json.loads((reference_dir / 'summary.json').read_text())
    rows = []
    for placement, organization, part_size in itertools.product(['interleaved', 'commit'], a.organizations, a.part_bytes):
        name = f'{placement}-{organization}-part{part_size}'
        inputs = a.output / (name + '-inputs')
        run([layout, trace, str(inputs), placement, organization, str(part_size)], a.output / (name + '-prepare'))
        result_dir = a.output / (name + '-matrix')
        run([sys.executable, str(matrix), replay, str(inputs / 'replay.tsv'), str(result_dir), '--cache-scope', 'shared', *policy_args], a.output / (name + '-run'))
        result = json.loads((result_dir / 'summary.json').read_text())
        canonical_entries = original_entry_keys(result['entry_pixel_domains'], Path(trace), inputs / 'replay.tsv')
        if result['pixel_domains'] != reference['pixel_domains'] or canonical_entries != [tuple(r) for r in reference['entry_pixel_domains']]:
            raise ValueError(f'{name}: decoded output differs from original layout')
        inventory = [{'path': str(p.relative_to(inputs)), 'bytes': p.stat().st_size, 'sha256': digest(p)}
                     for p in sorted(inputs.rglob('*')) if p.is_file()]
        (inputs / 'inventory.json').write_text(json.dumps(inventory, indent=2) + '\n')
        for cell in result['results']:
            rows.append(dict(cell, placement=placement, organization=organization, part_bytes=part_size))
        print(f'{name}: {len(result["results"])} policies, original pixels identical', flush=True)
    if source_hashes != {str(p.resolve()): digest(p) for p in sorted(source_paths)}:
        raise ValueError('original source changed during layout comparison')
    if digest(a.replay_binary) != identity['replay_sha256'] or digest(a.layout_binary) != identity['layout_sha256']:
        raise ValueError('program changed during comparison')
    (a.output / 'summary.json').write_text(json.dumps(dict(identity, pixel_domains=reference['pixel_domains'], entry_pixel_domains=reference['entry_pixel_domains'],
        original=reference['results'], results=rows), indent=2) + '\n')


if __name__ == '__main__':
    main()
