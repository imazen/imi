"""Separate transport overfetch from syntax cost on fixed real encoded images."""
import argparse
import csv
import hashlib
import itertools
import json
from pathlib import Path
import subprocess


def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('binary', type=Path)
    p.add_argument('geometry', type=Path)
    p.add_argument('output', type=Path)
    a = p.parse_args()
    a.output.mkdir(exist_ok=False)
    inputs = json.loads((a.geometry / 'results.json').read_text())
    sources = ['earth-level-16', 'webb-quarter', 'milkyway-level-32']
    cells = [r for r in inputs['results'] if r['source'] in sources
             and r['quality'] in ['lossless', '20', '90'] and r['outer_group'] in [0, 512]]
    if len(cells) != 18:
        raise ValueError('fixed 18-cell source domain')
    identity = {'binary_sha256': sha(a.binary), 'geometry_sha256': sha(a.geometry / 'results.json')}
    (a.output / 'identity.json').write_text(json.dumps(identity) + '\n')
    results = []
    for cell in cells:
        name = f"{cell['source']}-q{cell['quality']}-g{cell['outer_group']}"
        folder = a.output / name
        folder.mkdir()
        source = a.geometry / cell['source'] / f"q{cell['quality']}-g{cell['outer_group']}"
        first = (source / 'trace.tsv').read_text().splitlines()[0]
        trace = folder / 'cold.tsv'
        trace.write_text(first + '\n')
        pixel_domain = None
        for rtt, bandwidth, gap in itertools.product([160, 320], [1250000, 12500000], [4096, 65536, 262144]):
            prefix = folder / f'rtt{rtt}-bw{bandwidth}-gap{gap}'
            command = list(map(str, [a.binary.resolve(), trace.resolve(), prefix.resolve(), 'ranges',
                                    33554432, rtt, bandwidth, 8, 'batch', 'shared', gap, 1048576]))
            prefix.with_suffix('.command.json').write_text(json.dumps(command) + '\n')
            with prefix.with_suffix('.log').open('xb') as log:
                process = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=False)
            prefix.with_suffix('.rc').write_text(str(process.returncode) + '\n')
            process.check_returncode()
            with prefix.with_suffix('.views.tsv').open() as stream:
                rows = list(csv.DictReader(stream, delimiter='\t'))
            if len(rows) != 1:
                raise ValueError('cold view domain')
            row = rows[0]
            domain = (row['label'], row['decoded_bytes'], row['xxh3'])
            if pixel_domain is None:
                pixel_domain = domain
            elif pixel_domain != domain:
                raise ValueError('coalescing changed pixels')
            results.append({'source': cell['source'], 'quality': cell['quality'],
                            'group': cell['outer_group'], 'rtt': rtt, 'bandwidth': bandwidth,
                            'gap': gap, 'bytes': int(row['bytes']), 'requests': int(row['requests']),
                            'rounds': int(row['dependency_rounds']),
                            'network_ms': float(row['finish_ms']) - float(row['start_ms']),
                            'returned_pixels': int(row['decoded_bytes']) // 3})
        print(name, '12 policies: identical pixels PASS', flush=True)
    if sha(a.binary) != identity['binary_sha256']:
        raise ValueError('replay binary changed')
    (a.output / 'results.json').write_text(json.dumps({'identity': identity, 'results': results}, indent=2) + '\n')


if __name__ == '__main__':
    main()
