#!/usr/bin/env python3
"""Require complete exact access observations and summarize worst-of-three costs."""

import argparse
import hashlib
import json
import statistics
from pathlib import Path


STORES = {
    "kadis700k_944.parquet": "Parquet",
    "kadis944-rg0-zarr-rows.zarr": "Zarr rows",
    "kadis944-rg0-zarr-grid.zarr": "Zarr grid",
    "kadis944-rg0-blosc2-rows.b2nd": "Blosc2 rows",
    "features-rg00.safetensors.zst": "SafeTensors + Zstd",
}
REGION = [440, 32000, 64, 1024]


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def fields(line: str) -> dict[str, str]:
    return dict(part.split("=", 1) for part in line.split() if "=" in part)


def summarize(study: Path, region_study: Path | None = None) -> dict:
    log = (study.parent / f"{study.name}.log").read_text().splitlines()
    if not log or not log[-1].startswith("run-heavy: done rc=0 "):
        raise ValueError("the normal wrapper did not finish successfully")
    python_path = study / "python-results.jsonl"
    imij_path = study / "imij-full.txt"
    python_rows = [json.loads(line) for line in python_path.read_text().splitlines()]
    if len(python_rows) != 30:
        raise ValueError("expected thirty Python observations")
    if len({row["raw_sha256"] for row in python_rows}) != 1:
        raise ValueError("Python observations used different raw sources")
    grouped: dict[tuple[str, str], list[dict]] = {}
    for row in python_rows:
        store = STORES.get(Path(row["store"]).name)
        scope = "full" if row["region"] is None else "narrow"
        if store is None or (scope == "narrow" and row["region"] != REGION):
            raise ValueError("unexpected store or region")
        if (
            row["kind"] not in ("parquet", "zarr", "blosc2", "safetensors-zstd")
            or row["rows"] != 64000
            or row["features"] != 944
            or row["row_group"] != 0
            or row["hot_runs"] != 11
            or row["exact_float32_bits"] is not True
            or "backend_import_ns" not in row
        ):
            raise ValueError("an observation has a different shape or exactness")
        grouped.setdefault((scope, store), []).append(row)
    if set(grouped) != {
        (scope, store) for scope in ("full", "narrow") for store in STORES.values()
    } or any(len(rows) != 3 for rows in grouped.values()):
        raise ValueError("missing or duplicated format/scope observations")

    imij_lines = imij_path.read_text().splitlines()
    if len(imij_lines) != 3:
        raise ValueError("expected three IMIJ observations")
    imij_rows = [fields(line) for line in imij_lines]
    if sorted(int(row["trial"]) for row in imij_rows) != [1, 2, 3] or any(
        row.get("exact") != "true"
        or row.get("mode") != "full"
        or row.get("width") != "944"
        or row.get("height") != "64000"
        or row.get("threads") != "1"
        or row.get("hot_runs") != "11"
        for row in imij_rows
    ):
        raise ValueError("IMIJ observations have a different shape or exactness")
    if len({(row["file_bytes"], row["raw_bytes"]) for row in imij_rows}) != 1:
        raise ValueError("IMIJ observations used different input sizes")

    cells = {}
    for (scope, store), rows in grouped.items():
        if (
            len(
                {
                    (row["store_sha256"], row["store_bytes"], row["raw_sha256"])
                    for row in rows
                }
            )
            != 1
        ):
            raise ValueError("a stored input changed between trials")
        cells[f"{scope}/{store}"] = {
            "stored_bytes": rows[0]["store_bytes"],
            "backend_import_worst3_ns": max(row["backend_import_ns"] for row in rows),
            "first_open_plus_decode_median3_ns": statistics.median(
                row["open_ns"] + row["first_decode_ns"] for row in rows
            ),
            "first_open_plus_decode_worst3_ns": max(
                row["open_ns"] + row["first_decode_ns"] for row in rows
            ),
            "hot_median_worst3_ns": max(row["hot_median_ns"] for row in rows),
            "hot_individual_worst_ns": max(row["hot_worst_ns"] for row in rows),
            "observations": 3,
        }
    cells["full/IMIJ whole-image"] = {
        "stored_bytes": int(imij_rows[0]["file_bytes"]),
        "first_read_plus_decode_median3_ns": statistics.median(
            int(row["read_ns"]) + int(row["first_decode_ns"]) for row in imij_rows
        ),
        "first_read_plus_decode_worst3_ns": max(
            int(row["read_ns"]) + int(row["first_decode_ns"]) for row in imij_rows
        ),
        "hot_median_worst3_ns": max(int(row["hot_median_ns"]) for row in imij_rows),
        "hot_individual_worst_ns": max(int(row["hot_worst_ns"]) for row in imij_rows),
        "observations": 3,
    }
    if region_study is not None:
        region_log = (
            (region_study.parent / f"{region_study.name}.log").read_text().splitlines()
        )
        if not region_log or not region_log[-1].startswith("run-heavy: done rc=0 "):
            raise ValueError("indexed IMIJ region job did not finish successfully")
        region_rows = [
            fields(line)
            for line in (region_study / "results.txt").read_text().splitlines()
        ]
        if len(region_rows) != 6:
            raise ValueError("expected six indexed IMIJ region observations")
        for group in ("944x1024", "64x64"):
            rows = [row for row in region_rows if row.get("group") == group]
            if sorted(int(row["trial"]) for row in rows) != [1, 2, 3] or any(
                row.get("exact") != "true"
                or row.get("mode") != "probe"
                or row.get("width") != "944"
                or row.get("height") != "64000"
                or [
                    int(row[key]) for key in ("x", "y", "region_width", "region_height")
                ]
                != REGION
                or row.get("hot_runs") != "11"
                for row in rows
            ):
                raise ValueError(f"indexed IMIJ {group} observations differ")
            if len({row["file_bytes"] for row in rows}) != 1:
                raise ValueError("indexed IMIJ file size changed between trials")
            cells[f"narrow/IMIJ indexed {group}"] = {
                "stored_bytes": int(rows[0]["file_bytes"]),
                "first_open_plus_decode_median3_ns": statistics.median(
                    int(row["open_ns"]) + int(row["first_decode_ns"]) for row in rows
                ),
                "first_open_plus_decode_worst3_ns": max(
                    int(row["open_ns"]) + int(row["first_decode_ns"]) for row in rows
                ),
                "hot_median_worst3_ns": max(int(row["hot_median_ns"]) for row in rows),
                "hot_individual_worst_ns": max(
                    int(row["hot_worst_ns"]) for row in rows
                ),
                "open_reads": [int(row["open_reads"]) for row in rows],
                "first_decode_fetched_bytes": [int(row["first_bytes"]) for row in rows],
                "observations": 3,
            }
    return {
        "scope": "one 64,000 × 944 Float32 row group, bit-exact features only",
        "narrow_region": REGION,
        "measurement_limit": (
            "Warm host cache after input hashing; Python format readers perform I/O "
            "inside reads, while IMIJ hot decodes reuse a resident compressed byte slice. "
            "The indexed IMIJ region files use a bounded fixed-model encoder, "
            "not the whole-image effort-7.5 encoder. This does not measure "
            "end-to-end trainer load, cold device latency, or a 699,999-row "
            "full-table access."
        ),
        "run_heavy": log[-1],
        "python_results_sha256": sha256_file(python_path),
        "imij_results_sha256": sha256_file(imij_path),
        "indexed_imij_results_sha256": (
            sha256_file(region_study / "results.txt") if region_study else None
        ),
        "cells": dict(sorted(cells.items())),
    }


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("study", type=Path)
    parser.add_argument("--region-study", type=Path)
    args = parser.parse_args()
    print(
        json.dumps(summarize(args.study, args.region_study), indent=2, sort_keys=True)
    )
