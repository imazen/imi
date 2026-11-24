"""Compare one verified IQA group on an S3-compatible LAN store.

Credentials and endpoint come from the caller's environment. The output keeps
hashes and timings, never a presigned URL or credential-bearing command line.
"""

import argparse
import hashlib
import json
import os
import pathlib
import subprocess
import time


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def run(command, *, capture=True):
    result = subprocess.run(command, check=False, capture_output=capture, text=True)
    if result.returncode:
        # A presigned URL can occur in argv; never include it in an exception.
        raise RuntimeError(f"{pathlib.Path(command[0]).name} exited {result.returncode}")
    return result.stdout.strip() if capture else ""


def aws(endpoint, *args):
    return run(["aws", "--endpoint-url", endpoint, *args])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("group", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument("packer", type=pathlib.Path)
    parser.add_argument("range_client", type=pathlib.Path)
    parser.add_argument("bucket")
    parser.add_argument("prefix")
    args = parser.parse_args()
    endpoint = os.environ["ZEN_S3_ENDPOINT"]
    if not endpoint.startswith(("http://", "https://")):
        raise ValueError("invalid store endpoint")
    for name in ("AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"):
        if not os.environ.get(name):
            raise ValueError(f"missing {name}")
    args.output.mkdir(parents=True, exist_ok=False)
    files = sorted((args.group / "independent").glob("[0-9][0-9][0-9].imi"))
    if len(files) != 126 or [path.name for path in files] != [f"{i:03}.imi" for i in range(126)]:
        raise ValueError("expected complete 126-image I01 group")
    reference = args.group / "reference.imi"
    if not reference.is_file():
        raise ValueError("missing verified reference-page image")
    aws(endpoint, "s3api", "head-bucket", "--bucket", args.bucket)
    records = {"provenance": {"packer_sha256": digest(args.packer),
                               "range_client_sha256": digest(args.range_client),
                               "reference_source_sha256": digest(reference),
                               "independent_source_sha256": [digest(path) for path in files],
                               "zstd_version": run(["zstd", "--version"]),
                               "compression": "zstd -3 --long=27 -T1"}}
    for arm, inputs, indexed, key, expected in (
        ("independent", files, "plain", 251, files[-1]),
        ("reference", [reference], "indexed", 1, reference),
    ):
        listing = args.output / f"{arm}.txt"
        listing.write_text("".join(f"{path}\n" for path in inputs))
        archive = args.output / f"{arm}.imijset"
        started = time.perf_counter()
        pack = json.loads(run([str(args.packer), "pack", str(listing), str(archive), indexed]))
        pack_seconds = time.perf_counter() - started
        if pack["entries"] != len(inputs) or not pack["all_entries_identical"]:
            raise ValueError("archive pack/readback mismatch")
        compressed = args.output / f"{arm}.imijset.zst"
        started = time.perf_counter()
        run(["zstd", "-3", "--long=27", "-T1", "-q", "-o", str(compressed), str(archive)])
        compress_seconds = time.perf_counter() - started
        variants = {}
        for kind, path in (("archive", archive), ("whole_zstd", compressed)):
            object_key = f"{args.prefix.rstrip('/')}/{path.name}"
            started = time.perf_counter()
            aws(endpoint, "s3api", "put-object", "--bucket", args.bucket,
                "--key", object_key, "--body", str(path))
            put_seconds = time.perf_counter() - started
            head = json.loads(aws(endpoint, "s3api", "head-object", "--bucket", args.bucket,
                                  "--key", object_key))
            if head["ContentLength"] != path.stat().st_size:
                raise ValueError("stored object length differs")
            gets = []
            for trial in range(1, 4):
                fetched = args.output / f"fetched-{trial}-{path.name}"
                started = time.perf_counter()
                aws(endpoint, "s3api", "get-object", "--bucket", args.bucket,
                    "--key", object_key, str(fetched))
                get_seconds = time.perf_counter() - started
                if digest(fetched) != digest(path):
                    raise ValueError("full SeaweedFS GET differs from source")
                gets.append({"seconds": get_seconds, "sha256": digest(fetched)})
            variant = {"bytes": path.stat().st_size, "sha256": digest(path),
                       "object_key": object_key, "put_seconds": put_seconds,
                       "get_trials": gets, "worst_get_seconds": max(v["seconds"] for v in gets)}
            if kind == "archive":
                # Do not persist the short-lived, signed URL in evidence.
                url = aws(endpoint, "s3", "presign", f"s3://{args.bucket}/{object_key}",
                          "--expires-in", "900")
                ranges = []
                for _ in range(3):
                    started = time.perf_counter()
                    observation = json.loads(run([str(args.range_client), url, str(key), str(expected)]))
                    wall = time.perf_counter() - started
                    if observation["sha256"] != digest(expected):
                        raise ValueError("range-read entry differs from original image")
                    ranges.append({"wall_seconds": wall, "entry": observation})
                variant["range_trials"] = ranges
                variant["worst_range_wall_seconds"] = max(v["wall_seconds"] for v in ranges)
            else:
                started = time.perf_counter()
                restored = hashlib.sha256()
                with subprocess.Popen(["zstd", "-d", "-c", "-q", str(fetched)],
                                      stdout=subprocess.PIPE) as process:
                    for chunk in iter(lambda: process.stdout.read(1024 * 1024), b""):
                        restored.update(chunk)
                    if process.wait() != 0:
                        raise ValueError("whole-object decompression failed")
                variant["decompress_seconds"] = time.perf_counter() - started
                if restored.hexdigest() != digest(archive):
                    raise ValueError("decompressed archive differs")
                variant["restored_archive_sha256"] = restored.hexdigest()
            variants[kind] = variant
        records[arm] = {"source_count": len(inputs), "source_bytes": sum(p.stat().st_size for p in inputs),
                        "pack_seconds": pack_seconds, "compress_seconds": compress_seconds,
                        "pack": pack, "variants": variants}
        (args.output / "results.json").write_text(json.dumps(records, indent=2) + "\n")
    print(json.dumps(records, indent=2))


if __name__ == "__main__":
    main()
