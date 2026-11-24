#!/usr/bin/env python3
"""Verified bounded HTTP source download shared by the NASA demo preparers."""
import hashlib
import json
import os
import re
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

def download(url, path, chunk_bytes=1 << 20):
    if not 1 <= chunk_bytes <= 32 << 20:
        raise ValueError("source chunk size must be between 1 byte and 32 MiB")
    # Large upstream responses can close early. Fetch bounded verified spans;
    # every range must retain the same source identity and exact declared length.
    with urllib.request.urlopen(urllib.request.Request(url + "?imi=length", headers={"Range": "bytes=0-0"}), timeout=90) as r:
        extent = re.fullmatch(r"bytes 0-0/(\d+)", r.headers.get("Content-Range", ""))
        etag = r.headers.get("ETag")
        if r.status != 206 or not extent or not etag or len(r.read(2)) != 1:
            raise RuntimeError("NASA range/identity discovery failed")
        length = int(extent[1])
    attempts = []
    with path.open("xb") as out:
        out.truncate(length)
        def fetch(start):
            end = min(length, start + chunk_bytes) - 1
            for attempt in range(5):
                try:
                    request = urllib.request.Request(url + f"?imi={start}-{attempt}", headers={
                        "Range": f"bytes={start}-{end}", "If-Range": etag})
                    with urllib.request.urlopen(request, timeout=90) as r:
                        body = r.read(end - start + 2)
                        if (r.status != 206 or r.headers.get("ETag") != etag
                                or r.headers.get("Content-Range") != f"bytes {start}-{end}/{length}"
                                or len(body) != end - start + 1):
                            raise RuntimeError("NASA short/changed range response")
                    if os.pwrite(out.fileno(), body, start) != len(body):
                        raise RuntimeError("short local write")
                    return
                except Exception as error:
                    attempts.append({"offset": start, "attempt": attempt, "error": str(error)})
                    if attempt == 4:
                        raise
                    time.sleep(0.25 * (attempt + 1))
        try:
            with ThreadPoolExecutor(max_workers=6) as pool:
                for i, _ in enumerate(pool.map(fetch, range(0, length, chunk_bytes))):
                    if i % 64 == 0:
                        print(f"verified source spans {i + 1}/{(length + chunk_bytes - 1) // chunk_bytes}", flush=True)
        finally:
            path.with_suffix(".transport.json").write_text(json.dumps({"url": url, "etag": etag,
                "bytes": length, "retry_failures": attempts}, indent=2) + "\n")
    digest = hashlib.sha256()
    with path.open("rb") as f:
        while chunk := f.read(1 << 20):
            digest.update(chunk)
    return digest.hexdigest()


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument("url")
    parser.add_argument("output", type=Path)
    parser.add_argument("--chunk-bytes", type=int, default=1 << 20)
    args = parser.parse_args()
    if args.output.exists():
        raise FileExistsError(args.output)
    print(download(args.url, args.output, args.chunk_bytes))
