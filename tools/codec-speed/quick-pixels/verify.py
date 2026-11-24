"""Real and synthetic interoperability; Pillow is a test-only QOI decoder."""
import hashlib
import json
from pathlib import Path
import random
import subprocess
import sys
from PIL import Image, __version__ as pillow_version
from oracle import decode


def sha(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()


def run(argv, stem):
    stem.with_suffix('.command.json').write_text(json.dumps([str(s) for s in argv]))
    with stem.with_suffix('.log').open('x') as log:
        rc = subprocess.run(argv, stdout=log, stderr=subprocess.STDOUT).returncode
    stem.with_suffix('.rc').write_text(str(rc))
    if rc:
        raise ValueError((rc, stem))


def main(probe, fixtures, retained_qoi, output):
    output.mkdir(parents=True)
    manifest = json.loads((fixtures / 'manifest.json').read_text())
    cases = []
    for f in manifest:
        raw = fixtures / f['raw']
        if sha(raw) != f['sha256']:
            raise ValueError('input digest')
        cases.append((f['name'], raw, f['width'], f['height'], {'gray': 1, 'ga': 2, 'rgb': 3, 'rgba': 4}[f['channels']], {'u8': 1, 'u16': 2, 'f32': 4}[f['sample']]))
    rng = random.Random(928341)
    special = [0, 1, 255, 256, 65535, 0x7c00, 0x8000, 0xfc00, 0x7e5a, 0x7f800000, 0xff800000, 0x7fa12345, 0x80000000, 0x7ff0000000000000, 0xfff0000000000000, 0x7ff012345678abcd, 0x8000000000000000, 2**64-1]
    for size in (1, 2, 4, 8):
        for count in range(1, 5):
            values = bytearray()
            last = bytes(size*count)
            for i in range(257 * 3):
                if i % 5:
                    last = b''.join(((rng.getrandbits(64) if i % 3 else special[i % len(special)]) % 2**(8*size)).to_bytes(size, 'little') for _ in range(count))
                values.extend(last)
            name = f'bits-{size}-{count}'
            path = output / (name + '.raw')
            path.write_bytes(values)
            cases.append((name, path, 257, 3, count, size))
    prepared = []
    for name, raw, w, h, count, size in cases:
        geometry = output / (name + '.geometry')
        geometry.write_text(f'{w} {h} {count} {size}\n')
        for mode in ['wide'] + (['qoi'] if size == 1 and count >= 3 else []):
            target = output / f'{name}-{mode}.bin'
            run([probe, 'encode', raw, geometry, mode, target], output / f'{name}-{mode}-encode')
            original = raw.read_bytes()
            if mode == 'wide':
                result = decode(target.read_bytes())
            else:
                with Image.open(target) as im:
                    if im.size != (w, h):
                        raise ValueError('dimensions')
                    result = im.convert('RGB' if count == 3 else 'RGBA').tobytes()
            if result != original:
                raise ValueError(('independent pixels', name, mode))
            prepared.append({'image': name, 'mode': mode, 'bytes': target.stat().st_size, 'sha256': sha(target), 'raw_sha256': sha(raw), 'real': not name.startswith('bits-')})
    # Decode previously retained files made by zenbitmaps' existing QOI encoder.
    for name in ['1000', '1400', '1600']:
        src = retained_qoi / f'{name}-qoi-0.bin'
        dest = output / f'{name}-old-qoi-decoded.raw'
        run([probe, 'decode', src, dest], output / f'{name}-old-qoi-decode')
        if dest.read_bytes() != (fixtures / f'photo-{name}.raw').read_bytes():
            raise ValueError('retained QOI pixels')
    (output / 'prepared.json').write_text(json.dumps(prepared, indent=2))
    (output / 'summary.json').write_text(json.dumps({'probe_sha256': sha(probe), 'pillow': pillow_version, 'encoded_cases': len(prepared), 'retained_qoi_decodes': 3, 'manifest': manifest}, indent=2))
    print(f'PASS {len(prepared)} independent decodes; 3 retained QOI files')


if __name__ == '__main__':
    main(*(Path(a).resolve() for a in sys.argv[1:]))
