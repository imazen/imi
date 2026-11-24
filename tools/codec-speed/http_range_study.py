#!/usr/bin/env python3
"""Exercise one sealed imij-set entry through instrumented HTTP 206 responses."""

import argparse
import hashlib
import json
import re
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def sha(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("source", type=Path)
    parser.add_argument("source_manifest", type=Path)
    parser.add_argument("client", type=Path)
    parser.add_argument("client_sha256")
    parser.add_argument("latency_ms", type=int)
    parser.add_argument("output", type=Path)
    parser.add_argument("--direct-key", type=int)
    parser.add_argument("--expected-archive-sha256")
    args = parser.parse_args()
    require(args.latency_ms in (0, 50), "only the fixed 0/50 ms controls are defined")
    require(sha(args.client) == args.client_sha256, "HTTP client binary changed")
    source_root = args.source_manifest.parent
    source_name = args.source.relative_to(source_root).as_posix()
    records = {row["artifact"]: row for row in json.loads(args.source_manifest.read_text())}
    expected = records[source_name]
    source_sha = sha(args.source)
    require(source_sha == expected["sha256"] and args.source.stat().st_size == expected["bytes"],
            "sealed source entry changed")
    archive_sha = sha(args.archive)
    if args.expected_archive_sha256 is not None:
        require(archive_sha == args.expected_archive_sha256, "sealed archive changed")
    archive_len = args.archive.stat().st_size
    if args.direct_key is None:
        ordinal = int(args.source.stem)
        require(0 <= ordinal < 126, "I01 ordinal")
        key = ordinal * 2 + 1
        scope = "one I01 image"
    else:
        require(args.direct_key >= 0 and args.expected_archive_sha256 is not None,
                "direct key requires pinned archive")
        key = args.direct_key
        scope = "one explicitly keyed entry"
    args.output.mkdir(parents=True, exist_ok=False)

    etag = '"' + archive_sha + '"'
    requests = []
    guard = threading.Lock()

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, _format: str, *_args: object) -> None:
            pass

        def do_GET(self) -> None:
            if self.path != "/images.imijset":
                self.send_error(404)
                return
            match = re.fullmatch(r"bytes=(\d+)-(\d+)", self.headers.get("Range", ""))
            if match is None:
                self.send_error(400, "one explicit byte range required")
                return
            start, end = map(int, match.groups())
            if not (0 <= start <= end < archive_len):
                self.send_error(416, "range outside archive")
                return
            if self.headers.get("If-Match") not in (None, etag):
                self.send_error(412, "archive identity changed")
                return
            if self.headers.get("Accept-Encoding") != "identity":
                self.send_error(406, "identity encoding required")
                return
            with args.archive.open("rb") as stream:
                stream.seek(start)
                body = stream.read(end - start + 1)
            require(len(body) == end - start + 1, "server source truncated")
            time.sleep(args.latency_ms / 1000)
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{end}/{archive_len}")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("ETag", etag)
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
            with guard:
                requests.append({"start": start, "end": end, "body_bytes": len(body),
                                 "if_match": self.headers.get("If-Match"), "status": 206})

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    trials = []
    try:
        url = f"http://127.0.0.1:{server.server_port}/images.imijset"
        for trial in range(3):
            with guard:
                start_count = len(requests)
            command = [str(args.client), url, str(key), str(args.source)]
            completed = subprocess.run(command, text=True, capture_output=True, check=False)
            (args.output / f"trial-{trial + 1}.stdout").write_text(completed.stdout)
            (args.output / f"trial-{trial + 1}.stderr").write_text(completed.stderr)
            with guard:
                trace = requests[start_count:].copy()
            (args.output / f"trial-{trial + 1}-requests.json").write_text(
                json.dumps(trace, indent=2) + "\n")
            require(completed.returncode == 0, f"HTTP client trial {trial + 1} failed")
            result = json.loads(completed.stdout)
            require(result["key"] == key and result["sha256"] == source_sha
                    and result["entry_bytes"] == expected["bytes"], "HTTP entry identity")
            actual_requests = sum(result[field] for field in
                                  ("open_requests", "lookup_requests", "copy_requests"))
            actual_bytes = sum(result[field] for field in
                               ("open_body_bytes", "lookup_body_bytes", "copy_body_bytes"))
            require(len(trace) == actual_requests and sum(row["body_bytes"] for row in trace) == actual_bytes,
                    "server/client request accounting differs")
            require(trace[0]["if_match"] is None and all(row["if_match"] == etag for row in trace[1:]),
                    "HTTP representation was not pinned")
            trials.append(result)
    finally:
        server.shutdown()
        worker.join()
        server.server_close()

    require(sha(args.archive) == archive_sha and sha(args.source) == source_sha,
            "archive or source changed during study")
    summary = {
        "scope": f"{scope} from imij-set through localhost HTTP 206 with injected request delay",
        "archive_sha256": archive_sha,
        "archive_bytes": archive_len,
        "source_sha256": source_sha,
        "source_bytes": expected["bytes"],
        "source_manifest_sha256": sha(args.source_manifest),
        "client_sha256": args.client_sha256,
        "latency_ms_per_request": args.latency_ms,
        "trials": trials,
        "worst_of_three_ms": {phase: max(row[f"{phase}_ms"] for row in trials)
                              for phase in ("open", "lookup", "copy")},
    }
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"latency_ms": args.latency_ms,
                      "worst_of_three_ms": summary["worst_of_three_ms"],
                      "requests": [sum(row[field] for field in
                                       ("open_requests", "lookup_requests", "copy_requests"))
                                   for row in trials]}, sort_keys=True))


if __name__ == "__main__":
    main()
