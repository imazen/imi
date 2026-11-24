#!/usr/bin/env python3
"""Exact single-image access to frozen ZIP, tar and Parquet over HTTP 206."""

import argparse
import hashlib
import io
import json
import re
import subprocess
import sys
import threading
import time
import urllib.request
import zipfile
import tarfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pyarrow.parquet as pq


def sha(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


class RangeFile(io.RawIOBase):
    def __init__(self, url: str) -> None:
        self.url = url
        self.length = None
        self.etag = None
        self.position = 0
        self.cache_start = 0
        self.cache = b""
        self.requests = 0
        self.body_bytes = 0
        self._fetch(0, 0)

    def _fetch(self, start: int, end: int) -> bytes:
        headers = {"Range": f"bytes={start}-{end}", "Accept-Encoding": "identity"}
        if self.etag is not None:
            headers["If-Match"] = self.etag
        with urllib.request.urlopen(urllib.request.Request(self.url, headers=headers), timeout=10) as response:
            require(response.status == 206 and response.headers.get("Content-Encoding") is None,
                    "HTTP source did not return identity 206")
            match = re.fullmatch(rf"bytes {start}-{end}/(\d+)", response.headers.get("Content-Range", ""))
            require(match is not None, "Content-Range differs from request")
            total = int(match.group(1))
            require(total > end and (self.length is None or self.length == total), "representation length changed")
            tag = response.headers.get("ETag", "")
            require(len(tag) >= 3 and tag.startswith('"') and tag.endswith('"')
                    and (self.etag is None or self.etag == tag), "strong ETag changed")
            require(int(response.headers.get("Content-Length", "-1")) == end - start + 1,
                    "HTTP body length differs")
            body = response.read()
            require(len(body) == end - start + 1, "HTTP body truncated or extended")
        self.length = total
        self.etag = tag
        self.requests += 1
        self.body_bytes += len(body)
        return body

    def readable(self) -> bool:
        return True

    def seekable(self) -> bool:
        return True

    def tell(self) -> int:
        return self.position

    def seek(self, offset: int, whence: int = io.SEEK_SET) -> int:
        if whence == io.SEEK_CUR:
            offset += self.position
        elif whence == io.SEEK_END:
            offset += self.length
        elif whence != io.SEEK_SET:
            raise ValueError("unsupported seek mode")
        require(0 <= offset <= self.length, "seek outside container")
        self.position = offset
        return offset

    def read(self, size: int = -1) -> bytes:
        if size < 0:
            size = self.length - self.position
        size = min(size, self.length - self.position)
        parts = []
        while size:
            cached = self.position - self.cache_start
            if not (0 <= cached < len(self.cache)):
                if size >= 65536:
                    self.cache_start = self.position
                    fetch_size = min(size, 4 * 1024 * 1024)
                else:
                    self.cache_start = (self.position // 65536) * 65536
                    fetch_size = 65536
                self.cache = self._fetch(self.cache_start,
                                         min(self.cache_start + fetch_size - 1, self.length - 1))
                cached = self.position - self.cache_start
            count = min(size, len(self.cache) - cached)
            parts.append(self.cache[cached:cached + count])
            self.position += count
            size -= count
        return b"".join(parts)

    def readinto(self, output: bytearray) -> int:
        data = self.read(len(output))
        output[:len(data)] = data
        return len(data)


def client(kind: str, url: str, name: str, source: Path) -> None:
    expected = source.read_bytes()
    start = time.perf_counter()
    stream = RangeFile(url)
    if kind == "zip":
        archive = zipfile.ZipFile(stream)
    elif kind == "tar":
        archive = tarfile.open(fileobj=stream, mode="r:")
    elif kind == "parquet":
        archive = pq.ParquetFile(stream)
    else:
        raise ValueError("unknown container")
    open_ms = (time.perf_counter() - start) * 1000
    retrieve = time.perf_counter()
    if kind == "zip":
        actual = archive.read(name)
    elif kind == "tar":
        member = archive.getmember(name)
        actual = archive.extractfile(member).read()
    else:
        ordinal = int(Path(name).stem)
        group = ordinal // 16
        table = archive.read_row_group(group, columns=["key", "payload"])
        rows = list(zip(table["key"].to_pylist(), table["payload"].to_pylist()))
        selected = [payload for key, payload in rows if key == name]
        require(len(selected) == 1, "Parquet key missing or duplicate")
        actual = selected[0]
    retrieve_ms = (time.perf_counter() - retrieve) * 1000
    require(actual == expected, "retrieved image differs from sealed source")
    print(json.dumps({"kind": kind, "name": name, "sha256": hashlib.sha256(actual).hexdigest(),
                      "entry_bytes": len(actual), "open_ms": open_ms, "retrieve_ms": retrieve_ms,
                      "total_ms": (time.perf_counter() - start) * 1000,
                      "requests": stream.requests, "body_bytes": stream.body_bytes}, sort_keys=True))


def study(kind: str, container: Path, source: Path, manifest: Path, delay_ms: int, output: Path) -> None:
    require(kind in ("zip", "tar", "parquet") and delay_ms in (0, 50), "fixed study configuration")
    expected_sources = {row["artifact"]: row for row in json.loads(manifest.read_text())}
    source_row = expected_sources[source.relative_to(manifest.parent).as_posix()]
    source_sha = sha(source)
    require(source_sha == source_row["sha256"] and source.stat().st_size == source_row["bytes"],
            "sealed source changed")
    container_sha = sha(container)
    container_len = container.stat().st_size
    output.mkdir(parents=True, exist_ok=False)
    etag = '"' + container_sha + '"'
    requests = []
    lock = threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, _format: str, *_args: object) -> None:
            pass

        def do_GET(self) -> None:
            if self.path != "/container":
                self.send_error(404)
                return
            match = re.fullmatch(r"bytes=(\d+)-(\d+)", self.headers.get("Range", ""))
            if match is None:
                self.send_error(400)
                return
            first, last = map(int, match.groups())
            if not (0 <= first <= last < container_len):
                self.send_error(416)
                return
            if (self.headers.get("If-Match") not in (None, etag)
                    or self.headers.get("Accept-Encoding") != "identity"):
                self.send_error(412)
                return
            with container.open("rb") as file:
                file.seek(first)
                body = file.read(last - first + 1)
            require(len(body) == last - first + 1, "server source truncated")
            time.sleep(delay_ms / 1000)
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {first}-{last}/{container_len}")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("ETag", etag)
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
            with lock:
                requests.append({"start": first, "end": last, "body_bytes": len(body),
                                 "if_match": self.headers.get("If-Match"), "status": 206})

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    trials = []
    try:
        for number in range(1, 4):
            with lock:
                prior = len(requests)
            command = [sys.executable, __file__, "client", kind,
                       f"http://127.0.0.1:{server.server_port}/container", source.name, str(source)]
            process = subprocess.run(command, text=True, capture_output=True, check=False)
            (output / f"trial-{number}.stdout").write_text(process.stdout)
            (output / f"trial-{number}.stderr").write_text(process.stderr)
            with lock:
                trace = requests[prior:].copy()
            (output / f"trial-{number}-requests.json").write_text(json.dumps(trace, indent=2) + "\n")
            require(process.returncode == 0, f"{kind} retrieval failed on trial {number}")
            result = json.loads(process.stdout)
            require(result["sha256"] == source_sha and result["entry_bytes"] == source_row["bytes"],
                    "retrieved payload identity")
            require(len(trace) == result["requests"]
                    and sum(row["body_bytes"] for row in trace) == result["body_bytes"],
                    "HTTP request accounting mismatch")
            require(trace[0]["if_match"] is None and all(row["if_match"] == etag for row in trace[1:]),
                    "HTTP representation not pinned")
            trials.append(result)
    finally:
        server.shutdown()
        thread.join()
        server.server_close()
    require(sha(container) == container_sha and sha(source) == source_sha, "study inputs changed")
    summary = {"kind": kind, "source_sha256": source_sha, "source_bytes": source_row["bytes"],
               "container_sha256": container_sha, "container_bytes": container_len,
               "manifest_sha256": sha(manifest), "delay_ms": delay_ms, "trials": trials,
               "worst_of_three_total_ms": max(row["total_ms"] for row in trials)}
    (output / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"kind": kind, "delay_ms": delay_ms,
                      "worst_of_three_total_ms": summary["worst_of_three_total_ms"],
                      "requests": [row["requests"] for row in trials],
                      "body_bytes": [row["body_bytes"] for row in trials]}, sort_keys=True))


if __name__ == "__main__":
    argument = argparse.ArgumentParser(description=__doc__)
    argument.add_argument("mode", choices=("client", "study"))
    argument.add_argument("kind", choices=("zip", "tar", "parquet"))
    argument.add_argument("a")
    argument.add_argument("b")
    argument.add_argument("c")
    argument.add_argument("d", nargs="?")
    argument.add_argument("e", nargs="?")
    parsed = argument.parse_args()
    if parsed.mode == "client":
        require(parsed.d is None and parsed.e is None, "client arguments")
        client(parsed.kind, parsed.a, parsed.b, Path(parsed.c))
    else:
        require(parsed.d is not None and parsed.e is not None, "study arguments")
        study(parsed.kind, Path(parsed.a), Path(parsed.b), Path(parsed.c), int(parsed.d), Path(parsed.e))
