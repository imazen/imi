"""Measure indexed I01 pixel access to an immutable local SeaweedFS object.

The endpoint and credentials come from the environment. Signed URLs are never
written to the report or included in an error message.
"""

import argparse
import hashlib
import json
import os
import pathlib
import subprocess
import time


def sha(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def run(command):
    result = subprocess.run(command, capture_output=True, text=True, check=False)
    if result.returncode:
        raise RuntimeError(
            f"{pathlib.Path(command[0]).name} failed with rc {result.returncode}"
        )
    return result.stdout.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sealed_study", type=pathlib.Path)
    parser.add_argument("source_tsv", type=pathlib.Path)
    parser.add_argument("client", type=pathlib.Path)
    parser.add_argument("bucket")
    parser.add_argument("output", type=pathlib.Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    record = json.loads((args.sealed_study / "results.json").read_text())
    original = record["reference"]["variants"]["archive"]
    archive = args.sealed_study / "reference.imijset"
    if sha(archive) != original["sha256"]:
        raise ValueError("sealed reference archive changed")
    rows = [row.split("\t") for row in args.source_tsv.read_text().splitlines()]
    if len(rows) != 126 or rows[-1][0] != "I01_25_05.png":
        raise ValueError("I01 source table changed")
    expected = rows[-1][4]
    if len(expected) != 64:
        raise ValueError("missing source pixel hash")
    endpoint = os.environ["ZEN_S3_ENDPOINT"]
    key = original["object_key"]
    head = json.loads(
        run(
            [
                "aws",
                "--endpoint-url",
                endpoint,
                "s3api",
                "head-object",
                "--bucket",
                args.bucket,
                "--key",
                key,
            ]
        )
    )
    if head["ContentLength"] != original["bytes"]:
        raise ValueError("stored object length changed")
    whole_image = pathlib.Path(
        (args.sealed_study / "reference.txt").read_text().strip()
    )
    if not whole_image.is_file():
        raise ValueError("sealed complete group source is missing")
    url = run(
        [
            "aws",
            "--endpoint-url",
            endpoint,
            "s3",
            "presign",
            f"s3://{args.bucket}/{key}",
            "--expires-in",
            "900",
        ]
    )
    whole_copy_trials = []
    for _ in range(3):
        started = time.perf_counter()
        result = json.loads(run([str(args.client), url, "1", str(whole_image)]))
        elapsed = time.perf_counter() - started
        if result["entry_bytes"] != whole_image.stat().st_size or result[
            "sha256"
        ] != sha(whole_image):
            raise ValueError("whole-entry copy differs from sealed source")
        whole_copy_trials.append({"process_seconds": elapsed, "result": result})
    trials = []
    for _ in range(3):
        started = time.perf_counter()
        result = json.loads(
            run([str(args.client), url, "--indexed", "1", "125", expected])
        )
        elapsed = time.perf_counter() - started
        if (
            result["pixel_sha256"] != expected
            or result["decoded_bytes"] != 589824
            or result["width"] != 512
            or result["height"] != 384
        ):
            raise ValueError("indexed pixels differ from original")
        trials.append({"process_seconds": elapsed, "result": result})
    report = {
        "scope": "one final I01 distortion, source-exact decoded RGB8 pixels",
        "archive_sha256": original["sha256"],
        "client_sha256": sha(args.client),
        "source_table_sha256": sha(args.source_tsv),
        "whole_group_source_sha256": sha(whole_image),
        "whole_copy_trials": whole_copy_trials,
        "trials": trials,
    }
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(
        json.dumps(
            {
                "requests": [t["result"]["requests"] for t in trials],
                "body_bytes": [t["result"]["body_bytes"] for t in trials],
                "whole_copy_requests": [
                    sum(
                        t["result"][f"{phase}_requests"]
                        for phase in ["open", "lookup", "copy"]
                    )
                    for t in whole_copy_trials
                ],
                "whole_copy_body_bytes": [
                    sum(
                        t["result"][f"{phase}_body_bytes"]
                        for phase in ["open", "lookup", "copy"]
                    )
                    for t in whole_copy_trials
                ],
                "worst_process_seconds": max(t["process_seconds"] for t in trials),
            }
        )
    )


if __name__ == "__main__":
    main()
