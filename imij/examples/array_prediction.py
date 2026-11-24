#!/usr/bin/env python3
"""Exact Z/T prediction over independent scalar V4 chunks.

Correlated and independent synthetic controls isolate payload effects. The
whole residual chunk is an ordinary lossless V4 image. Prediction is external
to the codec. The reader reverses it before comparing storage bits.
"""
import argparse
import itertools
import json
from pathlib import Path

import numpy as np

from array_equivalence import Codec


SHAPE = (4, 8, 48, 64)


def inputs(shape):
    rng = np.random.default_rng(173311)
    t = np.arange(shape[0], dtype="<u8")[:, None, None, None]
    z = np.arange(shape[1], dtype="<u8")[None, :, None, None]
    origin = rng.integers(0, 1 << 40, size=shape[2:], dtype="<u8")
    noise = rng.integers(0, 4, size=shape, dtype="<u8")
    yield "u64-stack", (origin + z * 37 + t * 5 + noise).astype("<u8")
    yield "u64-independent", np.frombuffer(rng.bytes(8 * np.prod(shape)), "<u8").reshape(shape)
    xyz = rng.uniform(0.5, 2.0, size=shape[2:])
    small = rng.normal(scale=1e-5, size=shape)
    yield "f32-stack", (xyz + (t + z) * 1e-4 + small).astype("<f4")
    yield "f32-independent", np.frombuffer(rng.bytes(4 * np.prod(shape)), "<f4").reshape(shape)


def segments(words, axis, restart):
    lines = np.swapaxes(words, 0, axis)
    extent = restart if axis == 1 and restart else len(lines)
    for start in range(0, len(lines), extent):
        yield lines[start:start + extent]


def predict(data, axes, restart=0, anchored=False):
    unsigned = np.dtype(f"<u{data.dtype.itemsize}")
    words = data.copy(order="C").view(unsigned)
    operation = np.bitwise_xor if data.dtype.kind == "f" else np.subtract
    for axis in axes:
        for lines in segments(words, axis, restart):
            lines[1:] = operation(lines[1:], lines[:1] if anchored else lines[:-1])
    return words.view(data.dtype)


def invert(data, axes, restart=0, anchored=False):
    unsigned = np.dtype(f"<u{data.dtype.itemsize}")
    words = data.copy(order="C").view(unsigned)
    operation = np.bitwise_xor if data.dtype.kind == "f" else np.add
    for axis in reversed(axes):
        for lines in segments(words, axis, restart):
            if anchored:
                lines[1:] = operation(lines[1:], lines[:1])
            else:
                operation.accumulate(lines, axis=0, dtype=unsigned, out=lines)
    return words.view(data.dtype)


def crop(codec, output, native, local_t, local_z, axes, restart, anchored):
    _, nz, height, width = native.shape
    x, y, w, h = width // 3, height // 3, 16, 16
    t0 = 0 if 0 in axes else local_t
    z0 = (local_z // restart * restart if restart else 0) if 1 in axes else local_z
    times = range(t0, local_t + 1)
    depths = range(z0, local_z + 1)
    if anchored and local_z != z0:
        depths = (z0, local_z)
    rectangles = [
        (x, y + (t * nz + z) * height, w, h)
        for t, z in itertools.product(times, depths)
    ]
    reads, size = map(int, codec.run("read", output, codec.file,
                                   *(n for rect in rectangles for n in rect)))
    data = np.frombuffer(codec.file.read_bytes(), native.dtype).reshape(
        len(times), len(depths), h, w)
    assert invert(data, axes, anchored=anchored)[-1, -1].tobytes() == native[local_t, local_z, y:y+h, x:x+w].tobytes()
    return dict(planes=len(rectangles), source_reads=reads, fetched_bytes=size)


def measure(codec, directory, array, nt, nz, axes, restart, anchored, measure_roi):
    compressed = count = 0
    output = directory / "transfer.imi"
    width = array.shape[-1]
    rois = {}
    targets = {
        "first": (0, 0),
        "interior": (array.shape[0] // 2, array.shape[1] // 2 - 1),
        "last": (array.shape[0] - 1, array.shape[1] - 1),
    } if measure_roi else {}
    for t, z in itertools.product(range(0, array.shape[0], nt), range(0, array.shape[1], nz)):
        native = array[t:t + nt, z:z + nz]
        residual = predict(native, axes, restart, anchored)
        height = residual.size // width
        codec.file.write_bytes(residual.tobytes())
        codec.run("encode", native.dtype.str, width, height, codec.file, output)
        compressed += output.stat().st_size
        count += 1
        codec.run("read", output, codec.file, 0, 0, width, height)
        decoded = np.frombuffer(codec.file.read_bytes(), native.dtype).reshape(native.shape)
        assert invert(decoded, axes, restart, anchored).tobytes() == native.tobytes()
        for label, (row_t, row_z) in targets.items():
            if t <= row_t < t + nt and z <= row_z < z + nz:
                rois[label] = crop(codec, output, native, row_t - t, row_z - z, axes, restart, anchored)
    # Causal prefix for one XY sample in the most dependent plane.
    predecessors = (nt if 0 in axes else 1) * ((restart or nz) if 1 in axes else 1)
    if anchored:
        predecessors = min(2, predecessors)
    row = dict(encoded_bytes=compressed, objects=count, worst_plane_samples=predecessors)
    if measure_roi:
        assert rois.keys() == targets.keys()
        row["rois"] = rois
    return row


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("codec", type=Path)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--roi", action="store_true", help="larger chunks and exact cold ROI reads")
    args = parser.parse_args()
    args.directory.mkdir()
    codec = Codec(args.codec.resolve(), args.directory)
    try:
        shape = (4, 8, 128, 256) if args.roi else SHAPE
        for name, array in inputs(shape):
            for nt, nz in ((1, 1), (1, 8), (2, 4), (4, 8)):
                candidates = {"raw": ()}
                if nz > 1:
                    candidates["z"] = (1,)
                if nt > 1:
                    candidates.update(t=(0,), tz=(0, 1))
                modes = [(label, axes, 0, False) for label, axes in candidates.items()]
                if args.roi and nz > 2:
                    modes.append(("z2", (1,), 2, False))
                    modes.append(("z-anchor", (1,), 0, True))
                if args.roi and nz > 4:
                    modes.append(("z4", (1,), 4, False))
                    modes.append(("z-anchor4", (1,), 4, True))
                for label, axes, restart, anchored in modes:
                    row = measure(codec, args.directory, array, nt, nz, axes, restart, anchored, args.roi)
                    print(json.dumps(dict(input=name, shape=shape, t=nt, z=nz, predictor=label, **row)),
                          flush=True)
    finally:
        codec.close()


if __name__ == "__main__":
    main()
