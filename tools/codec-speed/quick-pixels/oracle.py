"""Independent IQP1 reconstruction from the documented word grammar."""
import struct


def decode(data):
    magic, w, h, count, size, reserved = struct.unpack_from('<4sIIBBH', data)
    if magic != b'IQP1' or not w or not h or count not in range(1, 5) or size not in (1, 2, 4, 8) or reserved:
        raise ValueError('header')
    if data[-8:] != bytes(7) + b'\1':
        raise ValueError('footer')
    mask = 2 ** (8 * size) - 1
    position = 16
    previous = [0, 0, 0, mask]
    slots = [[0] * 4 for _ in range(64)]
    output = bytearray()
    produced = 0
    while produced < w * h:
        op = data[position]
        position += 1
        run = 1
        if op in (254, 255):
            n = (1 if count < 3 else 3) + (op == 255)
            values = [int.from_bytes(data[position+i*size:position+(i+1)*size], 'little') for i in range(n)]
            position += n * size
            rgb = [values[0]] * 3 if count < 3 else values[:3]
            previous = rgb + [values[-1] if op == 255 else previous[3]]
        elif op < 64:
            previous = slots[op][:]
        elif op < 128:
            diffs = [(op // 16) % 4 - 2, (op // 4) % 4 - 2, op % 4 - 2]
            previous = [(previous[i] + diffs[i]) & mask for i in range(3)] + previous[3:]
        elif op < 192:
            t = data[position]
            position += 1
            g = op - 160
            diffs = [g + t // 16 - 8, g, g + t % 16 - 8]
            previous = [(previous[i] + diffs[i]) & mask for i in range(3)] + previous[3:]
        else:
            run = op - 191
        if position > len(data) - 8 or produced + run > w * h:
            raise ValueError('packet bounds')
        if count < 3 and previous[:3] != [previous[0]] * 3:
            raise ValueError('gray')
        folded = [sum(v.to_bytes(size, 'little')) for v in previous]
        index = sum(a * b for a, b in zip(folded, [3, 5, 7, 11])) % 64
        slots[index] = previous[:]
        components = previous[:count] if count >= 3 else [previous[0]] + (previous[3:] if count == 2 else [])
        pixel = b''.join(v.to_bytes(size, 'little') for v in components)
        output.extend(pixel * run)
        produced += run
    if position != len(data) - 8:
        raise ValueError('trailing bytes')
    return bytes(output)
