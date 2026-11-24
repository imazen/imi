#!/usr/bin/env python3
"""Compare immutable IMI-backed array slices with an independent dense ndarray."""
import argparse
import itertools
import json
import math
from pathlib import Path
import subprocess

import numpy as np


class Codec:
    def __init__(self, executable, directory):
        self.file = directory / "transfer.raw"
        self.process = subprocess.Popen(
            [executable], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True
        )

    def run(self, *args):
        self.process.stdin.write(" ".join(map(str, args)) + "\n")
        self.process.stdin.flush()
        answer = self.process.stdout.readline()
        if not answer:
            raise RuntimeError(f"codec exited while running {args[0]}")
        return answer.split()

    def close(self):
        self.process.stdin.close()
        if self.process.wait():
            raise RuntimeError("codec driver failed")


def write_chunks(codec, directory, array, chunk, fold, fill):
    directory.mkdir()
    grid = tuple((n + c - 1) // c for n, c in zip(array.shape, chunk))
    entries = {}
    dense = array.copy()
    for index, coord in enumerate(itertools.product(*(range(n) for n in grid))):
        bounds = tuple(slice(k * c, min((k + 1) * c, n))
                       for k, c, n in zip(coord, chunk, array.shape))
        if index % 11 == 7:
            dense[bounds] = fill
            continue
        pixels = array[bounds].ravel()
        width = min(fold, pixels.size)
        height = (pixels.size + width - 1) // width
        padded = np.zeros(width * height, dtype=array.dtype)
        padded[:pixels.size] = pixels
        codec.file.write_bytes(padded.tobytes())
        filename = f"{index}.imi"
        path = directory / filename
        codec.run("encode", array.dtype.str, width, height, codec.file, path)
        entries[".".join(map(str, coord))] = filename
    manifest = {
        "schema": "imij.array/v1-study", "shape": array.shape, "dtype": array.dtype.str,
        "chunk_shape": chunk, "order": "C", "fold_width": fold,
        "fill_le": np.asarray(fill, dtype=array.dtype).tobytes().hex(),
        "entries": entries,
    }
    (directory / "manifest.json").write_text(json.dumps(manifest) + "\n")
    return dense


def axis_groups(size, extent, selection):
    if isinstance(selection, int):
        index = selection + size if selection < 0 else selection
        if not 0 <= index < size:
            raise IndexError(index)
        indices = [index]
    else:
        indices = range(*selection.indices(size))
    groups = {}
    for out, index in enumerate(indices):
        tile, local = divmod(index, extent)
        groups.setdefault(tile, ([], []))
        groups[tile][0].append(out)
        groups[tile][1].append(local)
    return len(indices), groups


def rectangles(offsets, width):
    spans = {}
    for offset in offsets:
        y, x = divmod(int(offset), width)
        start, end = spans.get(y, (x, x))
        spans[y] = min(start, x), max(end, x)
    runs = []
    for y, (start, end) in sorted(spans.items()):
        w = end - start + 1
        if runs and runs[-1][0] == start and runs[-1][2] == w and runs[-1][1] + runs[-1][3] == y:
            runs[-1][3] += 1
        else:
            runs.append([start, y, w, 1])
    return runs


def read_slice(codec, directory, manifest, selectors):
    dtype = np.dtype(manifest["dtype"])
    fill = np.frombuffer(bytes.fromhex(manifest["fill_le"]), dtype=dtype)[0]
    shape, chunk = manifest["shape"], manifest["chunk_shape"]
    counts, by_axis = zip(*(axis_groups(n, c, sel)
                           for n, c, sel in zip(shape, chunk, selectors)))
    result = np.full(counts, fill, dtype=dtype)
    stats = dict(chunks=0, present=0, rectangles=0, source_reads=0, fetched_bytes=0,
                 returned_samples=result.size, samples_inside_rectangles=0)
    for coord in itertools.product(*by_axis):
        stats["chunks"] += 1
        filename = manifest["entries"].get(".".join(map(str, coord)))
        if not filename:
            continue
        stats["present"] += 1
        axes = [groups[k] for groups, k in zip(by_axis, coord)]
        extent = tuple(min(c, n - k * c) for n, c, k in zip(shape, chunk, coord))
        offsets = np.ravel_multi_index(np.ix_(*(group[1] for group in axes)), extent)
        width = min(manifest["fold_width"], math.prod(extent))
        runs = rectangles(offsets.ravel(), width)
        flat = [n for run in runs for n in run]
        reads, size = map(int, codec.run("read", directory / filename, codec.file, *flat))
        stats["rectangles"] += len(runs)
        stats["source_reads"] += reads
        stats["fetched_bytes"] += size
        returned = np.frombuffer(codec.file.read_bytes(), dtype=dtype)
        stats["samples_inside_rectangles"] += returned.size
        rows = {}
        at = 0
        for x0, y0, w, h in runs:
            for row in range(h):
                rows[y0 + row] = at + row * w - x0
            at += w * h
        packed = np.fromiter(
            (rows[y] + x for y, x in (divmod(int(n), width) for n in offsets.flat)),
            dtype=np.intp, count=offsets.size,
        )
        summary = returned[packed].reshape(offsets.shape)
        result[np.ix_(*(group[0] for group in axes))] = summary
    squeezed = tuple(0 if isinstance(sel, int) else slice(None) for sel in selectors)
    return result[squeezed], stats


def array_cells(codec, directory):
    rng = np.random.default_rng(70188)
    for dtype, shape, block in [
        ("<u8", (5, 47, 63), (2, 24, 32)),
        ("<i8", (4, 5, 31, 39), (2, 2, 16, 20)),
        ("<f4", (2, 3, 5, 27, 35), (1, 2, 2, 14, 18)),
        ("<u8", (1,) * 30 + (21, 29), (1,) * 30 + (11, 15)),
    ]:
        # Full-width integer and IEEE storage bits, including NaN payloads.
        raw_type = np.dtype(f"<u{np.dtype(dtype).itemsize}")
        data = rng.bytes(math.prod(shape) * raw_type.itemsize)
        array = np.frombuffer(data, dtype=dtype).reshape(shape)
        fill = np.frombuffer(bytes.fromhex("4523c17f"), dtype=dtype)[0] if dtype == "<f4" else array.flat[1]
        full = (slice(None),) * len(shape)
        mid = tuple(n // 2 for n in shape)
        for fold in (block[-1], 64, 1024):
            case = directory / f"{len(shape)}-{dtype[1:]}-{fold}"
            dense = write_chunks(codec, case, array, block, fold, fill)
            manifest = json.loads((case / "manifest.json").read_text())
            selectors = {
                "point": mid,
                "xy": (*mid[:-2], slice(3, -3), slice(5, -5)),
                "all-leading-one-xy": (*full[:-2], mid[-2], mid[-1]),
                "central-box": tuple(slice(n // 4, max(1, 3 * n // 4)) for n in shape),
                "reverse-strided": (*full[:-2], slice(None, None, -2), slice(1, None, 3)),
                "empty": (slice(0, 0), *full[1:]),
                "full": full,
            }
            for query, index in selectors.items():
                actual, stats = read_slice(codec, case, manifest, index)
                expected = dense[index]
                assert actual.shape == expected.shape
                assert actual.tobytes() == expected.tobytes(), (dtype, fold, query)
                yield {"shape": shape, "dtype": dtype, "fold": fold, "query": query, **stats}


def directory_control(codec, directory):
    catalog = json.loads((directory / "image.json").read_text())
    dataset = catalog["datasets"][0]
    rep = dataset["representations"]["lossless"]["full"]
    w, h = rep["width"], rep["height"]
    n, size, *offsets = map(int, codec.run(
        "view", directory / "lossless.imi", rep["viewId"], w, h, codec.file
    ))
    assert codec.file.read_bytes() == (directory / "source.rgb").read_bytes()
    assert "index" not in rep and "bootstrap" not in rep
    ranges = list(zip(offsets[::2], offsets[1::2]))
    assert ranges and all(0 <= lo < hi <= rep["encodedBytes"] for lo, hi in ranges)
    assert all(a[1] <= b[0] for a, b in zip(ranges, ranges[1:]))
    return {"native_full": [w, h], "file_bytes": rep["encodedBytes"], "extent_count": len(ranges),
            "range_bytes": sum(b-a for a, b in ranges), "source_reads": n, "fetched_bytes": size,
            "regenerated_extents": ranges}


def main():
    parser = argparse.ArgumentParser(__doc__)
    parser.add_argument("codec", type=Path)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--native-control", type=Path)
    args = parser.parse_args()
    args.directory.mkdir()
    codec = Codec(args.codec.resolve(), args.directory)
    try:
        for row in array_cells(codec, args.directory):
            print(json.dumps(row), flush=True)
        if args.native_control:
            print(json.dumps(directory_control(codec, args.native_control)), flush=True)
    finally:
        codec.close()


if __name__ == "__main__":
    main()
