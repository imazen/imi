"""Independent, deliberately scalar decoder for the experimental SPX1 payload."""
import struct


def decode(data):
    magic, width, height, channels, size, mode, reserved = struct.unpack_from('<4sIIBBBB', data)
    if magic != b'SPX1' or not width or not height or channels not in range(1, 5) or size not in (1, 2, 4, 8) or mode not in range(6) or reserved:
        raise ValueError('header')
    if mode == 4 and (channels, size) != (3, 1):
        raise ValueError('green mode')
    bpp, pos, rows = channels * size, 16, []
    rowbytes = width * bpp
    if rowbytes * height > 256 * 1024 * 1024:
        raise ValueError('experiment cap')
    for y in range(height):
        if mode == 0:
            row = data[pos:pos + rowbytes]
            pos += rowbytes
        else:
            record, = struct.unpack_from('<I', data, pos)
            pos += 4
            if record == 0:
                if not rows:
                    raise ValueError('first repeat')
                rows.append(rows[-1])
                continue
            payload = data[pos:pos + (record >> 1)]
            pos += record >> 1
            if len(payload) != record >> 1:
                raise ValueError('truncated')
            if not record & 1:
                row = payload
            elif mode == 2:
                row, p, previous = bytearray(), 0, bytes(bpp)
                while p < len(payload):
                    tag = payload[p]
                    p += 1
                    if tag < 128:
                        if bpp != 3:
                            raise ValueError('rgb')
                        if tag < 64:
                            delta = [(tag >> 4 & 3) - 2, (tag >> 2 & 3) - 2, (tag & 3) - 2]
                        else:
                            g, t = (tag & 63) - 32, payload[p]
                            p += 1
                            delta = [g + (t >> 4) - 8, g, g + (t & 15) - 8]
                        previous = bytes((v + d) % 256 for v, d in zip(previous, delta))
                        row.extend(previous)
                    elif tag < 192:
                        row.extend(previous * ((tag & 63) + 1))
                    else:
                        n = ((tag & 63) + 1) * bpp
                        row.extend(payload[p:p + n])
                        p += n
                        previous = bytes(row[-bpp:])
                    if len(row) > rowbytes:
                        raise ValueError('packet extent')
                if p != len(payload):
                    raise ValueError('packet bounds')
            elif mode >= 3:
                row, p = bytearray(rowbytes), 0
                for c in range(channels):
                    base = int.from_bytes(payload[p:p + size], 'little')
                    shift, bits = payload[p + size:p + size + 2]
                    p += size + 2
                    if shift > 63 or bits > 64 or bits and bits + shift > size * 8:
                        raise ValueError('packed bits')
                    n = (width * bits + 7) // 8
                    plane = payload[p:p + n]
                    if len(plane) != n:
                        raise ValueError('packed length')
                    # Whole-plane integer, structurally separate from Rust's byte reservoir.
                    integer = int.from_bytes(plane, 'little')
                    p += n
                    if integer >> (width * bits):
                        raise ValueError('nonzero padding')
                    mask = (1 << bits) - 1
                    for x in range(width):
                        value = base + (((integer >> (x * bits)) & mask) << shift)
                        at = x * bpp + c * size
                        row[at:at + size] = value.to_bytes(size, 'little')
                if p != len(payload):
                    raise ValueError('packed trailing')
                if mode == 4:
                    for x in range(0, rowbytes, 3):
                        row[x] = (row[x] + row[x + 1]) % 256
                        row[x + 2] = (row[x + 2] + row[x + 1]) % 256
                elif mode == 5:
                    for x in range(bpp, rowbytes):
                        row[x] ^= row[x - bpp]
            else:
                raise ValueError('record mode')
        if len(row) != rowbytes:
            raise ValueError('row length')
        rows.append(bytes(row))
    if pos != len(data):
        raise ValueError('trailing')
    return b''.join(rows)
