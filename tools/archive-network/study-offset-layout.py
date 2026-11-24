#!/usr/bin/env python3
"""Measure V4 page packing on consecutive records with published tile lengths."""
import argparse
import itertools
import json
import math
from pathlib import Path
import struct
import subprocess

import xxhash


def varint(value):
    out = bytearray()
    while value >= 128:
        out.append((value & 127) | 128)
        value >>= 7
    out.append(value)
    return bytes(out)


def integers(*values):
    return b"".join(map(varint, values))


def chunk_len(payload_size):
    return 4 + len(varint(payload_size + 1)) + payload_size


def read_index(path):
    raw = path.read_bytes()
    w, h, tw, th, count = struct.unpack_from("<IIIII", raw, 8)
    assert raw[:8] == b"IMITILE1" and len(raw) == 28 + 44 * count
    assert math.ceil(w / tw) * math.ceil(h / th) == count
    assert count
    entries = list(struct.iter_unpack("<QI32x", raw[28:]))
    gaps = [cur[0] - prev[0] - prev[1] for prev, cur in zip(entries, entries[1:])]
    assert all(gap >= 0 for gap in gaps) and all(length > 0 for _, length in entries)
    return entries, [gap for gap in gaps if gap]


def bit_pack(values, width):
    output = bytearray()
    bits = word = 0
    for value in values:
        assert value.bit_length() <= width
        word |= value << bits
        bits += width
        while bits >= 8:
            output.append(word & 255)
            word >>= 8
            bits -= 8
    if bits:
        output.append(word)
    return bytes(output)


def packed_relative(lengths):
    minimum = min(lengths)
    deltas = [n - minimum for n in lengths]
    width = max(deltas).bit_length()
    dense = bit_pack(deltas, width)
    bits = int.from_bytes(dense, "little")
    mask = (1 << width) - 1
    assert [minimum + ((bits >> (i * width)) & mask) for i in range(len(lengths))] == lengths
    return integers(minimum, width) + dense


class Packer:
    def __init__(self, binary):
        self.process = subprocess.Popen(
            [binary], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        )

    def __call__(self, body):
        return self.request(body, False)

    def qoi(self, body):
        return self.request(body, True)

    def request(self, body, qoi):
        self.process.stdin.write(struct.pack("<I", len(body) | ((1 << 31) if qoi else 0)) + body)
        self.process.stdin.flush()
        size, = struct.unpack("<I", self.process.stdout.read(4))
        packed = self.process.stdout.read(size)
        assert len(packed) == size
        return packed

    def close(self):
        self.process.stdin.close()
        if self.process.wait():
            raise RuntimeError("metadata pack witness failed")


def measure(entries, leaf_size, pack, legacy=False):
    # Current pREC lengths include each record's canonical ordinal. The
    # witness reuses legacy payload lengths, placing records consecutively.
    if legacy:
        bodies = [n for _, n in entries]
        offsets = [at for at, _ in entries] + [sum(entries[-1])]
        for first in range(0, len(bodies), leaf_size):
            page = entries[first:first + leaf_size]
            relative = list(itertools.accumulate((n for _, n in page), initial=page[0][0]))
            assert relative[:-1] == [at for at, _ in page]
    else:
        bodies = [n + len(varint(i)) for i, (_, n) in enumerate(entries)]
        frames = map(chunk_len, bodies)
        offsets = list(itertools.accumulate(frames, initial=0))
    root = bytearray(integers(1, len(bodies), leaf_size, offsets[-1]))
    if legacy:
        root.extend(varint(offsets[0]))
    for mode in ("v4", "adaptive", "qoi"):
        root_bytes = root.copy()
        leaves = []
        choices = 0
        for first in range(0, len(bodies), leaf_size):
            block = bodies[first:first + leaf_size]
            count = len(block)
            prefix = integers(first, count)
            varints = integers(*block)
            if mode == "v4":
                leaf = pack(prefix + varints)
            elif mode == "adaptive":
                plain = pack(prefix + varints)
                relative = pack(prefix + b"\x00" + packed_relative(block))
                choices += len(relative) < len(plain)
                leaf = min((plain, relative), key=len)
            else:
                lo, hi = min(block), max(block)
                if hi - lo >= 1 << 24:
                    raise ValueError("QOI witness requires 24-bit page residuals")
                pixels = b"".join((value - lo).to_bytes(3, "big") for value in block)
                leaf = pack(prefix + b"\x00" + integers(lo) + pack.qoi(pixels))
            leaves.append(chunk_len(len(leaf)))
            data_start, data_end = offsets[first], offsets[first + count]
            span = data_end - data_start
            digest = xxhash.xxh3_128(b"IMI/v4/plane-record-page\x00")
            digest.update(integers(first, count, data_start, span) + leaf)
            root_bytes.extend(integers(span, len(leaf)))
            root_bytes.extend(digest.intdigest().to_bytes(16, "little"))
        root_bytes.extend(b"\x00")  # no attachments
        ordered = sorted(leaves)
        packed_root = len(pack(root_bytes))
        yield dict(layout="legacy" if legacy else "native-model", mode=mode,
                   page_records=leaf_size, pages=len(leaves), bitpacked_pages=choices,
                   packed_root=packed_root, packed_leaves=sum(leaves),
                   total=packed_root + sum(leaves), leaf_p95=ordered[math.ceil(len(leaves)*.95)-1],
                   leaf_max=max(leaves), cold_root_plus_p95=packed_root + ordered[math.ceil(len(leaves)*.95)-1])


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("packer", type=Path)
    parser.add_argument("indices", nargs="+", type=Path)
    args = parser.parse_args()
    pack = Packer(args.packer)
    try:
        for index in args.indices:
            entries, gaps = read_index(index)
            print(json.dumps(dict(index=index.name, records=len(entries),
                                  legacy_offset_length_bytes=28 + 12 * len(entries),
                                  gaps=len(gaps), gap_bytes=sum(gaps), max_gap=max(gaps, default=0))))
            for n in (64, 256, 1024, 4096):
                for row in measure(entries, n, pack):
                    print(json.dumps(dict(index=index.name, **row)), flush=True)
            for row in measure(entries, 256, pack, legacy=True):
                print(json.dumps(dict(index=index.name, **row)), flush=True)
    finally:
        pack.close()


if __name__ == "__main__":
    main()
