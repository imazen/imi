#!/usr/bin/env python3
"""Inventory a frozen f32 feature Parquet without decoding its values."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path

import pyarrow.parquet as pq


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--features", type=int, required=True)
    args = parser.parse_args()

    source = pq.ParquetFile(args.source)
    fields = list(source.schema_arrow)
    feature_names = [f"f{i}" for i in range(args.features)]
    by_name = {field.name: field for field in fields}
    if len(by_name) != len(fields) or any(
        name not in by_name or str(by_name[name].type) != "float"
        for name in feature_names
    ):
        raise ValueError("feature columns are missing, duplicated or not Float32")
    if [field.name for field in fields[-args.features :]] != feature_names:
        raise ValueError("feature columns do not occupy the expected final block")

    digest = file_sha256(args.source)
    if digest != args.expected_sha256:
        raise ValueError(f"source SHA-256 mismatch: {digest}")
    meta = source.metadata
    groups = []
    for index in range(meta.num_row_groups):
        group = meta.row_group(index)
        feature_nulls = 0
        nonfeature_nulls = 0
        missing_null_statistics = []
        compression = Counter()
        for column_index in range(group.num_columns):
            column = group.column(column_index)
            compression[column.compression] += 1
            stats = column.statistics
            if stats is None or stats.null_count is None:
                missing_null_statistics.append(column.path_in_schema)
            elif column.path_in_schema in feature_names:
                feature_nulls += stats.null_count
            else:
                nonfeature_nulls += stats.null_count
        groups.append(
            {
                "index": index,
                "rows": group.num_rows,
                "compressed_column_bytes": sum(
                    group.column(column_index).total_compressed_size
                    for column_index in range(group.num_columns)
                ),
                "feature_compressed_bytes": sum(
                    group.column(column_index).total_compressed_size
                    for column_index in range(group.num_columns - args.features, group.num_columns)
                ),
                "feature_null_cells_from_statistics": feature_nulls,
                "nonfeature_null_cells_from_statistics": nonfeature_nulls,
                "missing_null_statistics": missing_null_statistics,
                "compression": dict(compression),
            }
        )
    if sum(group["rows"] for group in groups) != meta.num_rows:
        raise ValueError("row-group counts do not match the file count")
    result = {
        "source": str(args.source),
        "source_sha256": digest,
        "stored_bytes": args.source.stat().st_size,
        "rows": meta.num_rows,
        "columns": meta.num_columns,
        "features": args.features,
        "feature_type": "Float32",
        "other_columns": [
            {"name": field.name, "type": str(field.type), "nullable": field.nullable}
            for field in fields[: -args.features]
        ],
        "type_counts": dict(Counter(str(field.type) for field in fields)),
        "row_groups": groups,
        "scope": "Parquet footer/schema/statistics inventory only; raw Float32 bits, null masks and read speed are not verified by this tool",
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: result[key] for key in ("stored_bytes", "rows", "columns", "features", "type_counts", "source_sha256")}))


if __name__ == "__main__":
    main()
