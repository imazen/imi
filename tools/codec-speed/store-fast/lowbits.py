"""Synthetic precision controls derived from a real photo; not a low-color corpus."""
import hashlib
import json
import pathlib
import subprocess
import sys
from oracle import decode


def main(probe, imij, photo, output):
    output.mkdir()
    source = photo.read_bytes()
    if len(source) != 1024 * 1024 * 3:
        raise ValueError('expected frozen RGB photo')
    rows = []
    for bits in range(2, 8):
        code = bytes(v >> (8 - bits) for v in source[1::3])
        maxval = (1 << bits) - 1
        for mapping in ['lsb', 'shift', 'replicate', 'scale']:
            def expand(v):
                if mapping == 'lsb':
                    return v
                if mapping == 'shift':
                    return v << (8 - bits)
                if mapping == 'scale':
                    return (v * 255 + maxval // 2) // maxval
                return sum(((v >> (bits - 1 - (i % bits))) & 1) << (7 - i) for i in range(8))
            raw = bytes(map(expand, code))
            path = output / f'{bits}-{mapping}.raw'
            path.write_bytes(raw)
            encoded = path.with_suffix('.spx')
            cmd = [str(probe), str(path), '1024', '1024', '1', '1', '3', str(encoded)]
            result = subprocess.run(cmd, capture_output=True)
            path.with_suffix('.command.json').write_text(json.dumps(cmd))
            path.with_suffix('.stdout').write_bytes(result.stdout)
            path.with_suffix('.stderr').write_bytes(result.stderr)
            if result.returncode or decode(encoded.read_bytes()) != raw:
                raise ValueError('stored precision mismatch')
            sizes = {}
            for effort in [0, 7.5]:
                args = [str(imij), 'encode', '--raw', '1024', '1024', 'gray', 'u8', '--lossless', '--effort', str(effort)]
                result = subprocess.run(args, input=raw, capture_output=True)
                dst = output / f'{bits}-{mapping}-e{effort}.imi'
                dst.write_bytes(result.stdout)
                dst.with_suffix('.command.json').write_text(json.dumps(args))
                dst.with_suffix('.stderr').write_bytes(result.stderr)
                if result.returncode:
                    raise ValueError('imij encode')
                reconstructed = subprocess.run([str(imij), 'decode-raw'], input=result.stdout, capture_output=True)
                if reconstructed.returncode or reconstructed.stdout != raw:
                    raise ValueError('imij pixels')
                sizes[str(effort)] = len(result.stdout)
            rows.append({'bits': bits, 'mapping': mapping, 'raw_sha256': hashlib.sha256(raw).hexdigest(), 'stored_bytes': encoded.stat().st_size, 'imij_bytes': sizes})
    (output / 'summary.json').write_text(json.dumps({'source_sha256': hashlib.sha256(source).hexdigest(), 'probe_sha256': hashlib.sha256(probe.read_bytes()).hexdigest(), 'imij_sha256': hashlib.sha256(imij.read_bytes()).hexdigest(), 'pixels': 1024 * 1024, 'scope': 'synthetic quantized green channel, exact storage mapping controls only', 'rows': rows}, indent=2))

if __name__ == '__main__':
    main(*(pathlib.Path(a).resolve() for a in sys.argv[1:]))
