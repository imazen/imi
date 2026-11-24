"""Read-only opcode and framing inventory for retained QOI/SPX1 photographs."""
from collections import Counter
import hashlib
import json
from pathlib import Path
import struct
import sys
from oracle import decode as decode_stored


def sha(data):
    return hashlib.sha256(data).hexdigest()


def decode_qoi(data):
    magic, w, h, channels, colorspace = struct.unpack_from('>4sIIBB', data)
    if magic != b'qoif' or channels not in (3, 4) or colorspace > 1:
        raise ValueError('QOI header')
    index = [(0, 0, 0, 0)] * 64
    previous, pos, output = (0, 0, 0, 255), 14, bytearray()
    counts, sizes, pixels = Counter(), Counter(), Counter()
    while len(output) < w * h * channels:
        start = pos
        tag = data[pos]
        pos += 1
        count = 1
        if tag == 254:
            previous = (*data[pos:pos + 3], previous[3])
            pos += 3
            name = 'rgb'
        elif tag == 255:
            previous = tuple(data[pos:pos + 4])
            pos += 4
            name = 'rgba'
        elif tag >> 6 == 0:
            previous = index[tag & 63]
            name = 'index'
        elif tag >> 6 == 1:
            delta = [(tag >> 4 & 3) - 2, (tag >> 2 & 3) - 2, (tag & 3) - 2]
            previous = (*(v + d & 255 for v, d in zip(previous[:3], delta)), previous[3])
            name = 'diff'
        elif tag >> 6 == 2:
            t, green = data[pos], (tag & 63) - 32
            pos += 1
            delta = [green + (t >> 4) - 8, green, green + (t & 15) - 8]
            previous = (*(v + d & 255 for v, d in zip(previous[:3], delta)), previous[3])
            name = 'luma'
        else:
            name, count = 'run', (tag & 63) + 1
        index[sum(v * k for v, k in zip(previous, (3, 5, 7, 11))) % 64] = previous
        output.extend(bytes(previous[:channels]) * count)
        counts[name] += 1
        sizes[name] += pos - start
        pixels[name] += count
    if len(output) != w * h * channels or data[pos:] != bytes(7) + b'\1':
        raise ValueError('QOI final extent')
    if sum(sizes.values()) + 22 != len(data):
        raise ValueError('QOI inventory')
    return bytes(output), {'opcodes': dict(counts), 'bytes': dict(sizes), 'pixels': dict(pixels), 'framing': 22}


def stored_inventory(data):
    _, width, height, channels, sample_bytes, mode, _ = struct.unpack_from('<4sIIBBBB', data)
    if (channels, sample_bytes, mode) != (3, 1, 2):
        raise ValueError('expected RGB8 packet file')
    pos, counts, sizes, pixels = 16, Counter(), Counter(), Counter()
    for _ in range(height):
        record, = struct.unpack_from('<I', data, pos)
        pos += 4
        length = record >> 1
        if not record:
            counts['row_repeat'] += 1
            pixels['row_repeat'] += width
        elif not record & 1:
            counts['raw_row'] += 1
            sizes['raw_row'] += length
            pixels['raw_row'] += width
            pos += length
        else:
            end = pos + length
            while pos < end:
                tag = data[pos]
                pos += 1
                n = 1
                if tag >> 6 == 0:
                    name, amount = 'diff', 1
                elif tag >> 6 == 1:
                    name, amount = 'luma', 2
                    pos += 1
                elif tag >> 6 == 2:
                    name, amount, n = 'run', 1, (tag & 63) + 1
                else:
                    name, n = 'literal', (tag & 63) + 1
                    amount = 1 + n * 3
                    pos += n * 3
                counts[name] += 1
                sizes[name] += amount
                pixels[name] += n
            if pos != end:
                raise ValueError('SPX row inventory')
    if pos != len(data) or sum(sizes.values()) + 16 + 4 * height != len(data):
        raise ValueError('SPX inventory')
    return {'opcodes': dict(counts), 'bytes': dict(sizes), 'pixels': dict(pixels), 'framing': 16 + 4 * height}


def main(qoi, stored, fixtures, output):
    rows = []
    for name in ['1000', '1400', '1600']:
        q = (qoi / f'{name}-qoi-0.bin').read_bytes()
        s = (stored / f'photo-{name}-2.spx').read_bytes()
        raw = (fixtures / f'photo-{name}.raw').read_bytes()
        decoded, inventory = decode_qoi(q)
        if decoded != raw or decode_stored(s) != raw:
            raise ValueError('independent original pixels')
        rows.append({'image': name, 'source_sha256': sha(raw), 'qoi_sha256': sha(q), 'stored_sha256': sha(s), 'qoi_bytes': len(q), 'stored_bytes': len(s), 'qoi': inventory, 'stored': stored_inventory(s)})
    output.write_text(json.dumps(rows, indent=2))
    print(json.dumps(rows, indent=2))

if __name__ == '__main__':
    main(*(Path(s) for s in sys.argv[1:]))
