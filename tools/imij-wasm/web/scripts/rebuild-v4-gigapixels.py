#!/usr/bin/env python3
"""Rebuild and optionally upload V4 archives from a released Explorer catalog.

Each complete (scene, level, quality) leaves an atomic receipt. The published
JSON changes only for successful receipts. --publish writes immutable data
objects; a Pages deployment is a separate command.
"""
import argparse
import concurrent.futures as futures
import ctypes
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import time
from urllib.parse import urlsplit

ORIGIN = "https://gigapixels.imazen.org"
BUCKET = "gigapixels"
PART_SIZE = 32 << 20
SPLIT_OVER = 128 << 20
CACHE = "public,max-age=31536000,immutable"


class Hash128(ctypes.Structure):
    _fields_ = [("low", ctypes.c_uint64), ("high", ctypes.c_uint64)]


lib = ctypes.CDLL("libxxhash.so.0")
lib.XXH3_128bits.argtypes = [ctypes.c_char_p, ctypes.c_size_t]
lib.XXH3_128bits.restype = Hash128
lib.XXH3_64bits.argtypes = [ctypes.c_char_p, ctypes.c_size_t]
lib.XXH3_64bits.restype = ctypes.c_uint64
lib.XXH3_createState.restype = ctypes.c_void_p
lib.XXH3_freeState.argtypes = [ctypes.c_void_p]
lib.XXH3_128bits_reset.argtypes = [ctypes.c_void_p]
lib.XXH3_128bits_update.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t]
lib.XXH3_128bits_digest.argtypes = [ctypes.c_void_p]
lib.XXH3_128bits_digest.restype = Hash128


def digest_bytes(digest):
    return struct.pack("<QQ", digest.low, digest.high).hex()


def xxh3(data):
    return digest_bytes(lib.XXH3_128bits(data, len(data)))


def archive_digest(path):
    state = lib.XXH3_createState()
    if not state:
        raise MemoryError("XXH3 state")
    try:
        if lib.XXH3_128bits_reset(state):
            raise RuntimeError("XXH3 reset")
        with path.open("rb") as data:
            while chunk := data.read(1 << 20):
                if lib.XXH3_128bits_update(state, chunk, len(chunk)):
                    raise RuntimeError("XXH3 update")
        return digest_bytes(lib.XXH3_128bits_digest(state))
    finally:
        lib.XXH3_freeState(state)


def download(rep, path):
    if path.exists():
        if path.stat().st_size != rep["encodedBytes"]:
            raise ValueError(f"cached source length {path}")
        with path.open("rb") as data:
            if hashlib.file_digest(data, "sha256").hexdigest() != rep["sha256"]:
                raise ValueError(f"cached source digest {path}")
        return
    tmp = path.with_suffix(".partial")
    piece = path.with_suffix(".downloading")
    parts = rep.get("parts")
    size = rep["encodedBytes"]
    extent = size if parts is None else parts["size"]
    urls = [rep["url"]] if parts is None else [
        f"{parts['base']}/{i:06d}.bin" for i in range(parts["count"])
    ]
    with tmp.open("a+b") as target:
        have = target.tell()
        complete = len(urls) if have == size else have // extent
        target.truncate(min(size, complete * extent))
        for ordinal, url in enumerate(urls[complete:], start=complete):
            try:
                subprocess.run(["curl", "-fsS", "--retry", "2", "--connect-timeout",
                                "15", "--max-time", "300", "-o", str(piece), url],
                               check=True)
            except subprocess.CalledProcessError:
                parsed = urlsplit(url)
                if parsed.netloc != "gigapixels.imazen.org":
                    raise
                key = parsed.path.lstrip("/")
                aws("s3api", "get-object", "--bucket", BUCKET, "--key", key, str(piece))
            if piece.stat().st_size != min(extent, size - ordinal * extent):
                raise ValueError(f"legacy archive part length {path} at {ordinal}")
            with piece.open("rb") as source:
                shutil.copyfileobj(source, target)
            piece.unlink()
    with tmp.open("rb") as result:
        if (tmp.stat().st_size != rep["encodedBytes"]
                or hashlib.file_digest(result, "sha256").hexdigest() != rep["sha256"]):
            failed = tmp.with_suffix(f".failed-source-{time.time_ns()}")
            tmp.rename(failed)
            raise ValueError(f"legacy archive identity {path}, retained {failed}")
    tmp.rename(path)


def aws(*args, optional=False):
    cmd = ["aws", "--endpoint-url", "https://" + os.environ["R2_ACCOUNT_ID"] + ".r2.cloudflarestorage.com"]
    env = dict(os.environ, AWS_ACCESS_KEY_ID=os.environ["R2_ACCESS_KEY_ID"],
               AWS_SECRET_ACCESS_KEY=os.environ["R2_SECRET_ACCESS_KEY"], AWS_DEFAULT_REGION="auto")
    env.pop("AWS_SESSION_TOKEN", None)
    result = subprocess.run(cmd + list(args), env=env, capture_output=True, text=True)
    if optional and result.returncode and ("(404)" in result.stderr or "Not Found" in result.stderr):
        return None
    result.check_returncode()
    return json.loads(result.stdout) if result.stdout else {}


def put(path, key, digest):
    head = aws("s3api", "head-object", "--bucket", BUCKET, "--key", key, optional=True)
    if head is None:
        aws("s3api", "put-object", "--bucket", BUCKET, "--key", key, "--body", str(path),
            "--content-type", "application/octet-stream", "--cache-control", CACHE,
            "--metadata", "xxh3=" + digest)
        head = aws("s3api", "head-object", "--bucket", BUCKET, "--key", key)
    if head["ContentLength"] != path.stat().st_size or head["Metadata"].get("xxh3") != digest:
        raise ValueError(f"immutable R2 object collision {key}")
    return head["ETag"]


def publish(path, rep):
    identity = archive_digest(path)
    index = path.with_suffix(".index")
    size, sidecar = path.stat().st_size, index.read_bytes()
    index_id = xxh3(sidecar)
    prefix = f"d/{identity}"
    rep = dict(rep)
    for old in ("sha256", "url", "etag", "parts", "file", "encoder"):
        rep.pop(old, None)
    if rep.get("qualityTarget") is not None:
        rep.pop("rois", None)
    rep.update(contentId=identity, wireVersion=4, encodedBytes=size, multipart=False,
               index=dict(format="offsets-v2", url=f"{ORIGIN}/d/{index_id}/index.bin",
                          xxh3=index_id, bytes=len(sidecar)))
    if size <= SPLIT_OVER:
        key = f"{prefix}/archive.bin"
        rep["etag"] = put(path, key, identity)
        rep["url"] = f"{ORIGIN}/{key}"
    else:
        count = (size + PART_SIZE - 1) // PART_SIZE
        table = bytearray()
        workers = 6

        def upload_part(item):
            ordinal, part, digest = item
            put(part, f"{prefix}/p/{ordinal:06d}.bin", digest)
            part.unlink()

        with path.open("rb") as src, futures.ThreadPoolExecutor(workers) as pool:
            for start in range(0, count, workers):
                batch = []
                for ordinal in range(start, min(start + workers, count)):
                    chunk = src.read(PART_SIZE)
                    if len(chunk) != min(PART_SIZE, size - ordinal * PART_SIZE):
                        raise OSError(f"short part {ordinal} of {path}")
                    part = path.with_suffix(f".part-{ordinal:06d}")
                    part.write_bytes(chunk)
                    batch.append((ordinal, part, xxh3(chunk)))
                    table.extend(struct.pack("<Q", lib.XXH3_64bits(chunk, len(chunk))))
                for _ in pool.map(upload_part, batch):
                    pass
        table_path = path.with_suffix(".parts")
        table_path.write_bytes(table)
        key = f"{prefix}/parts.xxh3"
        put(table_path, key, xxh3(bytes(table)))
        rep["parts"] = dict(size=PART_SIZE, count=count, base=f"{ORIGIN}/{prefix}/p",
                            table=dict(url=f"{ORIGIN}/{key}", bytes=len(table),
                                       xxh3=f"{lib.XXH3_64bits(bytes(table), len(table)):016x}"))
    put(index, f"d/{index_id}/index.bin", index_id)
    return rep


def atomic_json(path, data):
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(data, separators=(",", ":")) + "\n")
    tmp.rename(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("catalog", type=Path)
    parser.add_argument("work", type=Path)
    parser.add_argument("encoder", type=Path)
    parser.add_argument("--select", action="append", default=[], help="SCENE/LEVEL; all if omitted")
    parser.add_argument("--jobs", type=int, default=2)
    parser.add_argument("--threads", type=int, default=8)
    parser.add_argument("--publish", action="store_true")
    parser.add_argument("--source-id", required=True, help="revision or source snapshot identity")
    args = parser.parse_args()
    args.work.mkdir(parents=True, exist_ok=True)
    catalog = json.loads(args.catalog.read_text())
    encoder_id = archive_digest(args.encoder)
    jobs = [(ds, level["key"]) for ds in catalog["datasets"] for level in ds["levels"]
            if not args.select or f"{ds['id']}/{level['key']}" in args.select]
    if not jobs or args.jobs < 1 or args.threads < 1:
        parser.error("choose existing levels and positive concurrency")

    def rebuild(job):
        ds, level = job
        work = args.work / ds["id"] / level
        work.mkdir(parents=True, exist_ok=True)
        original = ds["representations"]["lossless"][level]
        modes = ds["representations"]
        if args.publish and all((work / f"{name}.json").exists() for name in modes):
            return
        source = work / "source.bin"
        download(original, source)
        for name, levels in modes.items():
            rep = levels[level]
            mode = name if name == "lossless" else str(rep["qualityTarget"])
            out = work / f"{name}.imijset"
            receipt = work / f"{name}.json"
            ready = work / f"{name}.ready"
            if receipt.exists() and args.publish:
                continue
            if not ready.exists():
                # Partial output from a prior failed attempt must be retained.
                for partial in (out, out.with_suffix(".index")):
                    if partial.exists():
                        partial.rename(partial.with_suffix(f".failed-{time.time_ns()}"))
                subprocess.run([str(args.encoder.resolve()), str(source), str(original["width"]),
                                str(original["height"]), str(out), mode],
                               env=dict(os.environ, RAYON_NUM_THREADS=str(args.threads)), check=True)
                ready.touch()
            print(f"{ds['id']}/{level}/{name}: {rep['encodedBytes']} → {out.stat().st_size} bytes",
                  flush=True)
            if args.publish:
                new = publish(out, rep)
                new["quality"] = (f"V4 {name}, effort 10, "
                                  + ("source exact" if name == "lossless" else f"fixed partitions, quality {mode}, Fast decode"))
                new["encoder"] = {"source": args.source_id, "xxh3": encoder_id}
                atomic_json(receipt, new)

    with futures.ThreadPoolExecutor(args.jobs) as pool:
        for _ in pool.map(rebuild, sorted(jobs, key=lambda job: job[0]["representations"]["lossless"][job[1]]["encodedBytes"])):
            pass
    published = 0
    for ds, level in jobs:
        for name, levels in ds["representations"].items():
            receipt = args.work / ds["id"] / level / f"{name}.json"
            if receipt.exists():
                levels[level] = json.loads(receipt.read_text())
                published += 1
    if published:
        atomic_json(args.work / "catalog-v4.json", catalog)
    print(f"Published representation receipts: {published}/{sum(len(ds['representations']) for ds, _ in jobs)}")


if __name__ == "__main__":
    main()
