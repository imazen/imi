"""Reconcile full KADID size/reference/access observations into a compact report."""
import collections
import json
import pathlib
import sys
from fast_lossless import require, sha


def summarize(root, output, summary):
    result = json.loads(summary.read_text())
    require(result["full_dataset"] and result["references"] == 81 and result["distortions"] == 10125, "incomplete domain")
    baseline = next(row["bytes"] for row in result["arms"] if row["arm"] == "independent")
    png = result["original_png_bytes"] + result["original_complete_score_table_bytes"]
    rows = []
    for row in result["arms"]:
        rows.append({**row, "saving_vs_independent_percent": 100 * (1 - row["bytes"] / baseline),
                     "saving_vs_original_png_csv_percent": 100 * (1 - row["bytes"] / png),
                     "bits_per_pixel": 8 * row["bytes"] / result["pixels"]})
    access, selected = {}, {}
    for arm in ["pages", "reference", "previous"]:
        histogram = collections.Counter()
        per_type = collections.Counter()
        for number in range(1, 82):
            group = root / f"I{number:02}"
            references = {}
            for line in (group / f"{arm}-references.tsv").read_text().splitlines():
                index, reference = line.split("\t")
                references[int(index)] = None if reference == "-" else int(reference)
            require(set(references) == set(range(126)), "reference domain")
            for ordinal, reference in references.items():
                if reference is not None:
                    require(arm != "pages", "independent page references an image")
                    require(0 <= reference < ordinal, "reference dependency order")
                    require(reference == (0 if arm == "reference" else ordinal - 1), "wrong policy")
                    per_type[(ordinal - 1) // 5 + 1] += 1
            records = [line.split("\t") for line in (group / f"{arm}-access.tsv").read_text().splitlines()]
            require([int(row[0]) for row in records] == list(range(5, 126, 5)), "access sample domain")
            for ordinal, count in records:
                decoded = 0
                cursor = int(ordinal)
                while cursor is not None:
                    decoded += 1
                    cursor = references[cursor]
                require(decoded == int(count), "access chain mismatch")
                histogram[decoded] += 1
        expected = next(row["selected_references"] for row in result["arms"] if row["arm"] == arm)
        require(sum(per_type.values()) == expected, "selected reference mismatch")
        require(sum(histogram.values()) == 81 * 25, "access sample count")
        access[arm] = {"targets": sum(histogram.values()),
                       "total_image_decodes": sum(k * v for k, v in histogram.items()),
                       "max_dependency_inclusive_decodes": max(histogram),
                       "histogram": dict(sorted(histogram.items()))}
        selected[arm] = {"per_type_opportunities": 405, "selected_by_distortion_type": dict(sorted(per_type.items()))}
    report = {"source_summary_sha256": sha(summary), "pixels": result["pixels"],
              "original_png_csv_bytes": png, "rows": rows, "access": access, "selection": selected,
              "scope": "all 81 groups; exact decoded RGB8 samples and item attributes; no PNG ancillary roundtrip"}
    with output.open("x") as stream:
        json.dump(report, stream, indent=2)
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    require(len(sys.argv) == 4, "iqa_summary.py FULL-STUDY NEW-OUTPUT.json SUMMARY.json")
    summarize(*(pathlib.Path(p) for p in sys.argv[1:]))
