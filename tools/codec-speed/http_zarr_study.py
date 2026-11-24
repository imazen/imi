#!/usr/bin/env python3
"""Fetch only the indexed Zarr v3 shard objects needed for one IMIJ payload."""

import argparse
import hashlib
import json
import re
import subprocess
import sys
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import zarr


def sha(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def content_index(results: Path) -> dict[str, tuple[int, str]]:
    stored = json.loads(results.read_text())["outputs"]["zarr_v3_uint8_shards"]
    rows = [line.split("\t") for line in stored["content_manifest"].splitlines()]
    require(len(rows) == stored["files"] and all(len(row) == 3 for row in rows), "Zarr inventory")
    return {name: (int(length), digest) for name, length, digest in rows}


def fetch(base: str, name: str, expected: tuple[int, str]) -> bytes:
    url = base + "/" + name
    with urllib.request.urlopen(
        urllib.request.Request(url, headers={"Accept-Encoding": "identity"}), timeout=10
    ) as response:
        require(response.status == 200 and response.headers.get("Content-Encoding") is None,
                "Zarr object response")
        require(int(response.headers.get("Content-Length", "-1")) == expected[0], "Zarr object length")
        require(response.headers.get("ETag") == '"' + expected[1] + '"', "Zarr object ETag")
        data = response.read()
    require(len(data) == expected[0] and hashlib.sha256(data).hexdigest() == expected[1],
            "Zarr object content")
    return data


def client(base: str, name: str, source: Path, inventory: Path) -> None:
    expected = source.read_bytes()
    files = content_index(inventory)
    start = time.perf_counter()
    metadata_bytes = fetch(base, "zarr.json", files["zarr.json"])
    metadata = json.loads(metadata_bytes)
    selected = [row for row in metadata["attributes"]["entries"] if row["key"] == name]
    require(len(selected) == 1, "Zarr key missing or duplicate")
    row = selected[0]
    first = row["offset"]
    last = first + row["length"]
    require(row["length"] == len(expected), "Zarr declared payload length")
    outer = metadata["chunk_grid"]["configuration"]["chunk_shape"][0]
    require(outer == 1024 * 1024 and metadata["codecs"][0]["name"] == "sharding_indexed",
            "unexpected Zarr shard layout")
    keys = [f"c/{n}" for n in range(first // outer, (last - 1) // outer + 1)]
    open_ms = (time.perf_counter() - start) * 1000
    retrieve = time.perf_counter()
    buffer = zarr.core.buffer.cpu.Buffer
    objects = {"zarr.json": buffer.from_bytes(metadata_bytes)}
    body_bytes = len(metadata_bytes)
    for key in keys:
        value = fetch(base, key, files[key])
        body_bytes += len(value)
        objects[key] = buffer.from_bytes(value)
    array = zarr.open_array(store=zarr.storage.MemoryStore(objects), mode="r")
    actual = array[first:last].tobytes()
    retrieve_ms = (time.perf_counter() - retrieve) * 1000
    require(actual == expected, "Zarr payload differs from sealed source")
    print(json.dumps({"kind": "zarr_v3_uint8_shards", "name": name,
                      "sha256": hashlib.sha256(actual).hexdigest(), "entry_bytes": len(actual),
                      "open_ms": open_ms, "retrieve_ms": retrieve_ms,
                      "total_ms": (time.perf_counter() - start) * 1000,
                      "requests": 1 + len(keys), "body_bytes": body_bytes,
                      "shard_keys": keys}, sort_keys=True))


def study(root: Path, source: Path, source_manifest: Path, inventory: Path,
          latency_ms: int, output: Path) -> None:
    require(latency_ms in (0, 50), "fixed latency controls")
    row = {entry["artifact"]: entry for entry in json.loads(source_manifest.read_text())}[
        source.relative_to(source_manifest.parent).as_posix()]
    source_sha = sha(source)
    require(source_sha == row["sha256"] and source.stat().st_size == row["bytes"],
            "sealed source changed")
    files = content_index(inventory)
    require(all(sha(root / name) == digest and (root / name).stat().st_size == length
                for name, (length, digest) in files.items()), "sealed Zarr objects changed")
    output.mkdir(parents=True, exist_ok=False)
    requests = []
    lock = threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, _format: str, *_args: object) -> None:
            pass

        def do_GET(self) -> None:
            name = self.path.removeprefix("/")
            if name not in files or (name != "zarr.json" and re.fullmatch(r"c/\d+", name) is None):
                self.send_error(404)
                return
            if self.headers.get("Accept-Encoding") != "identity":
                self.send_error(406)
                return
            body = (root / name).read_bytes()
            length, digest = files[name]
            require(len(body) == length and hashlib.sha256(body).hexdigest() == digest,
                    "server Zarr object changed")
            time.sleep(latency_ms / 1000)
            self.send_response(200)
            self.send_header("Content-Length", str(length))
            self.send_header("ETag", '"' + digest + '"')
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
            with lock:
                requests.append({"object": name, "body_bytes": length, "status": 200})

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    trials = []
    try:
        for number in range(1, 4):
            with lock:
                prior = len(requests)
            process = subprocess.run(
                [sys.executable, __file__, "client", f"http://127.0.0.1:{server.server_port}",
                 source.name, str(source), str(inventory)],
                text=True, capture_output=True, check=False,
            )
            (output / f"trial-{number}.stdout").write_text(process.stdout)
            (output / f"trial-{number}.stderr").write_text(process.stderr)
            with lock:
                trace = requests[prior:].copy()
            (output / f"trial-{number}-requests.json").write_text(json.dumps(trace, indent=2) + "\n")
            require(process.returncode == 0, f"Zarr retrieval failed on trial {number}")
            result = json.loads(process.stdout)
            require(result["sha256"] == source_sha and result["entry_bytes"] == row["bytes"],
                    "Zarr payload identity")
            require([item["object"] for item in trace] == ["zarr.json", *result["shard_keys"]]
                    and sum(item["body_bytes"] for item in trace) == result["body_bytes"],
                    "Zarr request accounting")
            trials.append(result)
    finally:
        server.shutdown()
        thread.join()
        server.server_close()
    require(sha(source) == source_sha and all(sha(root / name) == digest
            for name, (_, digest) in files.items()), "study inputs changed")
    summary = {"source_sha256": source_sha, "source_bytes": row["bytes"],
               "zarr_manifest_sha256": sha(inventory), "delay_ms": latency_ms,
               "trials": trials, "worst_of_three_total_ms": max(row["total_ms"] for row in trials)}
    (output / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"kind": "zarr_v3_uint8_shards", "delay_ms": latency_ms,
                      "worst_of_three_total_ms": summary["worst_of_three_total_ms"],
                      "requests": [row["requests"] for row in trials],
                      "body_bytes": [row["body_bytes"] for row in trials]}, sort_keys=True))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("client", "study"))
    parser.add_argument("a")
    parser.add_argument("b")
    parser.add_argument("c")
    parser.add_argument("d")
    parser.add_argument("e", nargs="?")
    parser.add_argument("f", nargs="?")
    args = parser.parse_args()
    if args.mode == "client":
        require(args.e is None and args.f is None, "client arguments")
        client(args.a, args.b, Path(args.c), Path(args.d))
    else:
        require(args.e is not None and args.f is not None, "study arguments")
        study(Path(args.a), Path(args.b), Path(args.c), Path(args.d), int(args.e), Path(args.f))
