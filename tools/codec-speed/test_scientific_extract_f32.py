#!/usr/bin/env python3
"""Check that the Parquet extractor preserves signed zero and NaN payload bits."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile

import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq


def float_array(bits: list[int], nulls: list[bool] | None = None) -> pa.Array:
    values = np.asarray(bits, dtype="<u4")
    valid = None
    if nulls is not None:
        valid = pa.py_buffer(bytes([sum(1 << index for index, ok in enumerate(nulls) if ok)]))
    return pa.Array.from_buffers(pa.float32(), len(bits), [valid, pa.py_buffer(values)])


def run(extractor: Path, source: Path, output: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [
            sys.executable,
            str(extractor),
            str(source),
            str(output),
            "--features",
            "2",
            "--groups",
            "0",
            "--source-sha256",
            "selftest",
        ],
        capture_output=True,
        text=True,
        check=False,
    )


def main() -> None:
    extractor = Path(__file__).with_name("scientific_extract_f32.py")
    root = Path(tempfile.mkdtemp(prefix="imij-f32-extract-", dir=Path.home() / "tmp"))
    source = root / "source.parquet"
    first = [0x80000000, 0x7FC01234, 0xFFC05678, 0x3F800000]
    second = [0x00000000, 0x7F800000, 0xFF800000, 0x00000001]
    pq.write_table(
        pa.table({"row_id": pa.array([1, 2, 3, 4]), "f0": float_array(first), "f1": float_array(second)}),
        source,
        row_group_size=4,
    )
    actual_first = pq.read_table(source, columns=["f0"])["f0"].chunk(0).buffers()[1]
    if bytes(actual_first) != np.asarray(first, dtype="<u4").tobytes():
        raise AssertionError("Parquet reader changed the test pattern before extraction")
    passed = run(extractor, source, root / "good")
    if passed.returncode:
        raise AssertionError(passed.stderr)
    actual = np.fromfile(root / "good/features-rg00.f32le", dtype="<u4").reshape(4, 2)
    expected = np.column_stack((first, second)).astype("<u4")
    if not np.array_equal(actual, expected):
        raise AssertionError("row-major raw Float32 bits changed")
    manifest = json.loads((root / "good/manifest.json").read_text())
    if manifest["blocks"][0]["bytes"] != 32:
        raise AssertionError("wrong block length")

    pq.write_table(
        pa.table({"f0": float_array(first, [True, False, True, True]), "f1": float_array(second)}),
        source,
        row_group_size=4,
    )
    rejected = run(extractor, source, root / "null")
    if rejected.returncode == 0 or "has 1 nulls" not in rejected.stderr:
        raise AssertionError("null feature was not refused")
    print(json.dumps({"raw_bits": 8, "signed_zero": True, "nan_payloads": 2, "null_refused": True}))


if __name__ == "__main__":
    main()
