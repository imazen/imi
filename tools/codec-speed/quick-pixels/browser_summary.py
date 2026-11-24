"""Validate and summarize the existing browser production probes."""
import hashlib
import json
from pathlib import Path
import sys


def require(ok, why):
    if not ok:
        raise ValueError(why)


def main(decode, encode, output, bit_smarts=False):
    output.mkdir()
    manifest = json.loads((decode / 'fixtures/manifest.json').read_text())
    records = {}
    versions = set()
    for block in range(3):
        lines = (decode / f'decode-{block}.log').read_text().splitlines()
        packet = json.loads(next(s.removeprefix('observations=') for s in lines if s.startswith('observations=')))
        versions.add(packet['version'])
        seen = set()
        for row in packet['rows']:
            key = row['name'], row['variant'], row['threads']
            require(key not in seen, 'duplicate browser cell')
            seen.add(key)
            image = next(f for f in manifest if f['name'] == row['name'])
            require(row['hash'] == image['sha256'], 'decoded pixels')
            require((row['width'], row['height']) == (image['width'], image['height']), 'geometry')
            records.setdefault(key, []).append({**row, 'category': image['category']})
        require(len(seen) == len(manifest) * 10, 'full variant/thread product')
    require(len(versions) == 1 and all(len(v) == 3 for v in records.values()), 'three blocks, same browser')
    groups = {}
    for (name, variant, threads), rows in records.items():
        key = rows[0]['category'], name.rsplit('/', 1)[1], variant, threads
        groups.setdefault(key, []).append({'pixels': rows[0]['width'] * rows[0]['height'],
                                          'worker_ms': max(r['workerMs'] for r in rows),
                                          'caller_ms': max(r['endToEndMs'] for r in rows)})
    decoded = [{'category': k[0], 'mode': k[1], 'variant': k[2], 'threads': k[3],
                **{a + '_mpx_s': sum(v['pixels'] for v in values) / sum(v[a + '_ms'] for v in values) / 1000 for a in ('worker', 'caller')}}
               for k, values in groups.items()]
    packet = json.loads((encode / 'summary.json').read_text())
    require(packet['complete'] and packet['cells'] == packet['planned'] == (40 if bit_smarts else 24), 'complete encode product')
    groups = {}
    for i, row in enumerate(packet['rows']):
        data = (encode / f'cell-{i}.imi').read_bytes()
        require(len(data) == row['encodedBytes'] and hashlib.sha256(data).hexdigest() == row['encodedSha'], 'encoded identity')
        image = next(f for f in manifest if f['raw'] == row['image'])
        require(row['decodedSha'] == image['sha256'], 'encode roundtrip')
        require(len(row['browserTimes']) == len(row['nativeTimes']) == 3, 'encode repetitions')
        key = row['category'], ('quick-'+row['bitSmarts'] if 'bitSmarts' in row else 'quick') if row['mode'] == 'quick' else str(row['effort'])
        groups.setdefault(key, []).append(row)
    encoded = [{'category': k[0], 'mode': k[1],
                'bpp': 8 * sum(r['encodedBytes'] for r in rows) / sum(r['width'] * r['height'] for r in rows),
                **{a + '_mpx_s': sum(r['width'] * r['height'] for r in rows) / sum(max(r[a + 'Times']) for r in rows) / 1000 for a in ('browser', 'native')}}
               for k, rows in groups.items()]
    result = {'browser': next(iter(versions)), 'decode_observations': sum(map(len, records.values())), 'encode_cells': packet['cells'],
              'encode_ms': packet['totalEncodeMs'], 'decode': decoded, 'encode': encoded}
    (output / 'summary.json').write_text(json.dumps(result, indent=2))
    print(json.dumps({k: result[k] for k in ('browser', 'decode_observations', 'encode_cells', 'encode_ms')}))


if __name__ == '__main__':
    args = sys.argv[1:]
    bit_smarts = args[-1:] == ['--bit-smarts']
    if bit_smarts: args.pop()
    main(*(Path(v).resolve() for v in args), bit_smarts=bit_smarts)
