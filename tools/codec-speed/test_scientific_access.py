#!/usr/bin/env python3
"""Exercise the four stored-format read paths with exact Float32 bit patterns."""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

import blosc2
import numpy as np
import pyarrow as pa
import pyarrow.parquet as pq
import safetensors.numpy
import zarr
import zstandard

from scientific_access import sha256_file, store_digest


def main() -> None:
    bits = np.array(
        [
            0,
            0x80000000,
            0x3F800000,
            0x7FC00001,
            0xFF800000,
            0x00000001,
            0xBF000000,
            0x7F7FFFFF,
            0x3EAAAAAB,
            0x00000000,
            0xFFFFFFFF,
            0x00800000,
            0x40490FDB,
            0xC0490FDB,
            0x3F000000,
            0x7F800000,
        ],
        dtype="<u4",
    ).reshape(4, 4)
    floats = bits.view("<f4")
    with tempfile.TemporaryDirectory(dir=Path.home() / "tmp") as scratch:
        root = Path(scratch)
        raw = root / "raw.f32le"
        raw.write_bytes(bits.tobytes())
        stores = {
            "parquet": root / "features.parquet",
            "zarr": root / "features.zarr",
            "blosc2": root / "features.b2nd",
            "safetensors-zstd": root / "features.safetensors.zst",
        }
        pq.write_table(
            pa.table({f"f{i}": pa.array(floats[:, i]) for i in range(4)}),
            stores["parquet"],
            row_group_size=4,
        )
        zarr_array = zarr.create_array(
            store=str(stores["zarr"]),
            shape=(4, 4),
            dtype="<f4",
            chunks=(4, 4),
            compressors=[
                zarr.codecs.BloscCodec(cname="zstd", clevel=5, shuffle="bitshuffle")
            ],
            zarr_format=3,
        )
        zarr_array[:] = floats
        blosc2.asarray(
            floats,
            chunks=(4, 4),
            blocks=(2, 2),
            cparams={
                "codec": blosc2.Codec.ZSTD,
                "clevel": 5,
                "filters": [blosc2.Filter.BITSHUFFLE],
                "nthreads": 1,
            },
            urlpath=str(stores["blosc2"]),
            contiguous=True,
        )
        stores["safetensors-zstd"].write_bytes(
            zstandard.ZstdCompressor(level=5).compress(
                safetensors.numpy.save({"features": floats})
            )
        )
        script = Path(__file__).with_name("scientific_access.py")
        for kind, store in stores.items():
            base_command = [
                sys.executable,
                str(script),
                kind,
                str(raw),
                str(store),
                "--rows",
                "4",
                "--features",
                "4",
                "--raw-sha256",
                sha256_file(raw),
                "--store-sha256",
                store_digest(store)[0],
                "--hot-runs",
                "1",
            ]
            for region in ([], ["--region", "1", "1", "2", "2"]):
                result = subprocess.run(
                    base_command + region, capture_output=True, text=True, check=True
                )
                report = json.loads(result.stdout)
                if not report["exact_float32_bits"] or report["kind"] != kind:
                    raise AssertionError(f"{kind} lost exact Float32 bits")
                if region and report["region"] != [1, 1, 2, 2]:
                    raise AssertionError(f"{kind} lost region identity")
            wrong_hash = base_command.copy()
            wrong_hash[wrong_hash.index("--store-sha256") + 1] = "0" * 64
            rejected = subprocess.run(wrong_hash, capture_output=True, text=True)
            if (
                rejected.returncode == 0
                or "stored representation identity differs" not in rejected.stderr
            ):
                raise AssertionError(
                    f"{kind} admitted a different stored representation"
                )
            print(
                f"{kind}: full/region exact Float32 bits and mismatched-store rejection",
                flush=True,
            )


if __name__ == "__main__":
    if not (Path.home() / "tmp").is_dir():
        raise SystemExit("~/tmp must exist")
    main()
