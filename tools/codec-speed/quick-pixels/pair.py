"""Same-binary, single-image band comparison using zenbench PR33 and independent reads."""
import gzip
import hashlib
import io
import json
from pathlib import Path
import struct
import subprocess
import sys
from PIL import Image, __version__ as pillow_version
from oracle import decode as wide_decode


def sha(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()


def execute(argv, path):
    path.with_suffix('.command.json').write_text(json.dumps([str(x) for x in argv]))
    with path.open('x') as log:
        rc = subprocess.run(argv, stdout=log, stderr=subprocess.STDOUT).returncode
    path.with_suffix('.rc').write_text(str(rc))
    if rc:
        raise ValueError((rc, path))


def independent(path, f, rows, wide):
    encoded = path.read_bytes()
    channels = dict(gray=1, ga=2, rgb=3, rgba=4)[f['channels']]
    size = dict(u8=1, u16=2, f32=4)[f['sample']]
    pieces = []
    if rows:
        if struct.unpack('<4s5I', encoded[:24]) != (b'QB01', f['width'], f['height'], channels, size, rows):
            raise ValueError('bundle header')
        pos = 24
        for y in range(0, f['height'], rows):
            n, = struct.unpack('<Q', encoded[pos:pos+8])
            pos += 8
            pieces.append((encoded[pos:pos+n], min(rows, f['height']-y)))
            pos += n
        if pos != len(encoded):
            raise ValueError('bundle extent')
    else:
        pieces = [(encoded, f['height'])]
    result = []
    for piece, height in pieces:
        if wide:
            raw = wide_decode(piece)
        else:
            with Image.open(io.BytesIO(piece)) as im:
                if im.size != (f['width'], height):
                    raise ValueError('independent band geometry')
                raw = im.convert('RGB' if channels == 3 else 'RGBA').tobytes()
        if len(raw) != f['width'] * height * channels * size:
            raise ValueError('independent band size')
        result.append(raw)
    if hashlib.sha256(b''.join(result)).hexdigest() != f['sha256']:
        raise ValueError('independent pixels')
    return len(pieces)


def main(probe, fixtures, zenbench, output):
    output.mkdir(parents=True)
    manifest = json.loads((fixtures / 'manifest.json').read_text())
    shared = [f for f in manifest if f['sample'] == 'u8' and f['channels'] in ('rgb', 'rgba')]
    arms = [(c, t, r) for c in ('original', 'zenbitmaps') for t, r in [(1, 0), (1, 64), (2, 64), (4, 64), (8, 64)]]
    def name(c, t, r):
        return f'{c}-{r}-{t}'
    prepared = []
    for f in manifest:
        raw = fixtures / f['raw']
        if sha(raw) != f['sha256']:
            raise ValueError('input identity')
        geometry = output / (f['name'] + '.geometry')
        geometry.write_text(f"{f['width']} {f['height']} {dict(gray=1,ga=2,rgb=3,rgba=4)[f['channels']]} {dict(u8=1,u16=2,f32=4)[f['sample']]}\n")
        configurations = ([*arms] if f in shared else []) + [('wide', t, 64) for t in (1, 2, 4, 8)]
        identity = {}
        for c, t, r in configurations:
            target = output / f"{f['name']}-{name(c,t,r)}.bin"
            execute([probe, 'prepare', c, str(t), str(r), raw, geometry, target], target.with_suffix('.log'))
            bands = independent(target, f, r, c == 'wide')
            key = c, r
            h = sha(target)
            if key in identity and identity[key] != h:
                raise ValueError('thread count changed encoded bytes')
            identity[key] = h
            prepared.append({'image': f['name'], 'arm': name(c,t,r), 'bytes': target.stat().st_size, 'sha256': h, 'independent_bands': bands})
    (output / 'prepared.json').write_text(json.dumps(prepared, indent=2))
    (output / 'identities.json').write_text(json.dumps({'probe': sha(probe), 'zenbench': sha(zenbench), 'manifest': manifest, 'pillow': pillow_version, 'arms': arms}, indent=2))
    def arm(c, t, r, action):
        return {'id': name(c,t,r), 'program': str(probe), 'args': [action, c, str(t), str(r), '${raw}', '${geometry}', '-'], 'timeout_seconds': 30, 'revision': sha(probe)}
    for action in ('encode', 'decode'):
        plan = {'schema': 1, 'baseline': arm(*arms[0], action), 'candidates': [arm(*a, action) for a in arms[1:]],
                'cases': [{'id': f['name'], 'inputs': {'raw': str(fixtures / f['raw']), 'geometry': str(output / (f['name'] + '.geometry'))}} for f in shared],
                'metric': {'extract': {'kind': 'prefix', 'prefix': action + '_ns='}, 'unit': 'ns', 'expected': 1, 'divide_by': 1},
                'check': {'kind': 'json', 'prefix': 'check=', 'pointers': ['/raw_sha256']},
                'warmup': 'One encode; both cross-decoders validate outside clocks. Decode has three additional same-route warmups and ten allocating measured decodes. Pool creation and IO excluded. Encode includes band collection/framing; decode includes ordered raster assembly.',
                'blocks': 3, 'aa': True, 'deadline_seconds': 180}
        path = output / f'{action}.json'
        path.write_text(json.dumps(plan, indent=2))
        execute([zenbench, 'pair', path, '--output', output / action], output / f'{action}.log')
        packet = json.loads(gzip.decompress((output / action / 'packet.json.gz').read_bytes()))
        if not packet['complete']:
            raise ValueError('incomplete measurement')
    encode_total = sum(int(line.split('=')[1]) for path in output.rglob('*') if path.is_file() and (path.suffix == '.log' or path.name == 'stdout.txt') for line in path.read_text().splitlines() if line.startswith('encode_total_ns='))
    (output / 'complete.json').write_text(json.dumps({'prepared_files': len(prepared), 'independent_bands': sum(p['independent_bands'] for p in prepared), 'encode_seconds': encode_total/1e9}, indent=2))
    if encode_total > 120e9:
        raise ValueError('encode budget exceeded')


if __name__ == '__main__':
    main(*(Path(v).resolve() for v in sys.argv[1:]))
