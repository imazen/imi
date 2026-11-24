"""Exact byte-plane prototype, three fresh observations on original rasters."""
import json
from pathlib import Path
import sys
from pair import execute, sha


def main(probe, fixtures, geometry_root, output):
    output.mkdir()
    manifest = json.loads((fixtures / 'manifest.json').read_text())
    rows = []
    for f in manifest:
        raw = fixtures / f['raw']
        if sha(raw) != f['sha256']:
            raise ValueError('source identity')
        for variant in ('packets', 'simple'):
            for block in range(3):
                stem = f"{f['name']}-{variant}-{block}"
                encoded = output / (stem + '.planes')
                log = output / (stem + '.log')
                args = [probe, raw, geometry_root / (f['name'] + '.geometry'), encoded]
                if variant == 'simple':
                    args.append('simple')
                execute(args, log)
                result = json.loads(log.read_text().splitlines()[-1])
                rows.append({'image': f['name'], 'variant': variant, 'block': block, 'raw_sha256': sha(raw),
                             'encoded_sha256': sha(encoded), **result})
    (output / 'results.json').write_text(json.dumps({'program_sha256': sha(probe), 'rows': rows}, indent=2))


if __name__ == '__main__':
    main(*(Path(v).resolve() for v in sys.argv[1:]))
