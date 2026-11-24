#!/usr/bin/env python3
"""Measure equal-result imij-set summary queries over strict HTTP byte ranges."""

import argparse
import hashlib
import json
import os
import re
import shlex
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
    parser.add_argument("archive_sha256")
    parser.add_argument("client", type=Path)
    parser.add_argument("client_sha256")
    parser.add_argument("latency_ms", type=int)
    parser.add_argument("output", type=Path)
    parser.add_argument("--trials", type=int, choices=(1, 3), default=3)
    parser.add_argument("--query", choices=("dimensions", "cicp"), default="dimensions")
    parser.add_argument("--remote-host")
    parser.add_argument("--remote-client")
    parser.add_argument("--advertise-host")
    parser.add_argument("--listen-port", type=int)
    args = parser.parse_args()
    require(args.latency_ms in (0, 50), "only fixed 0/50 ms controls are defined")
    require(sha(args.archive) == args.archive_sha256, "archive changed")
    require(sha(args.client) == args.client_sha256, "client changed")
    remote = args.remote_host is not None
    if remote:
        require(args.remote_client is not None and args.advertise_host is not None
                and args.listen_port is not None and 3000 <= args.listen_port <= 3999,
                "remote run needs client, advertised host and port 3000-3999")
        remote_sha = subprocess.run(
            ["ssh", "-o", "BatchMode=yes", args.remote_host,
             shlex.join(["sha256sum", args.remote_client])],
            text=True, capture_output=True, check=True)
        require(remote_sha.stdout.split()[0] == args.client_sha256,
                "remote client differs from sealed local binary")
    else:
        require(args.remote_client is None and args.advertise_host is None
                and args.listen_port is None, "remote options require remote host")
    args.output.mkdir(parents=True, exist_ok=False)
    archive_len = args.archive.stat().st_size
    etag = '"' + args.archive_sha256 + '"'
    requests: list[dict] = []
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
                self.send_error(400, "one byte range required")
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
            require(len(body) == end - start + 1, "archive truncated")
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
                                 "client_address": self.client_address[0],
                                 "if_match": self.headers.get("If-Match"), "status": 206})

    server = ThreadingHTTPServer(("0.0.0.0" if remote else "127.0.0.1",
                                  args.listen_port if remote else 0), Handler)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    observations = []
    try:
        host = args.advertise_host if remote else "127.0.0.1"
        url = f"http://{host}:{server.server_port}/images.imijset"
        for mode in ("pruned", "scan"):
            for trial in range(args.trials):
                with guard:
                    before = len(requests)
                query_args = (["--dimensions", mode, "512", "940", "256"]
                              if args.query == "dimensions" else
                              ["--cicp", mode, "1", "13", "0", "1", "256"])
                client_command = [str(args.remote_client if remote else args.client),
                                  url, *query_args]
                if remote:
                    # REMOTE_WRAPPER: an optional command prefix on the remote host
                    # (for example a heavy-job slot and memory cap), split like a shell.
                    wrapper = shlex.split(os.environ.get("REMOTE_WRAPPER", ""))
                    command = ["ssh", "-o", "BatchMode=yes", args.remote_host,
                               shlex.join([*wrapper, *client_command])]
                else:
                    command = client_command
                completed = subprocess.run(command, text=True, capture_output=True, check=False)
                name = f"{mode}-{trial + 1}"
                (args.output / f"{name}.stdout").write_text(completed.stdout)
                (args.output / f"{name}.stderr").write_text(completed.stderr)
                with guard:
                    trace = requests[before:].copy()
                (args.output / f"{name}-requests.json").write_text(
                    json.dumps(trace, indent=2) + "\n")
                require(completed.returncode == 0, f"{name} failed: {completed.stderr}")
                result = json.loads(completed.stdout)
                require(result["mode"] == mode and result["query"] == args.query
                        and result["count"] == 256,
                        "query scope differs")
                require(len(trace) == result["open_requests"] + result["query_requests"],
                        "request accounting differs")
                require(sum(row["body_bytes"] for row in trace) ==
                        result["open_body_bytes"] + result["query_body_bytes"],
                        "body byte accounting differs")
                require(trace[0]["if_match"] is None and
                        all(row["if_match"] == etag for row in trace[1:]),
                        "HTTP representation was not pinned")
                if remote:
                    require(all(row["client_address"] != "127.0.0.1" for row in trace),
                            "remote client was served through loopback")
                observations.append(result)
    finally:
        server.shutdown()
        worker.join()
        server.server_close()

    expected_keys = hashlib.sha256(b"".join(key.to_bytes(8, "little") for key in range(256))).hexdigest()
    require(all(row["key_sha256"] == expected_keys for row in observations),
            "query results differ from independent key list")
    require(sha(args.archive) == args.archive_sha256 and sha(args.client) == args.client_sha256,
            "input changed during study")
    summary = {"scope": "million-entry synthetic archive over separate-host LAN HTTP 206"
               if remote else "million-entry synthetic archive over localhost HTTP 206",
               "query": args.query,
               "archive_sha256": args.archive_sha256, "archive_bytes": archive_len,
               "client_sha256": args.client_sha256, "latency_ms": args.latency_ms,
               "client_host": args.remote_host if remote else "localhost",
               "server_host": args.advertise_host if remote else "localhost",
               "trials_per_mode": args.trials,
               "expected_key_sha256": expected_keys, "observations": observations,
               "max_query_ms": {mode: max(row["query_ms"] for row in observations
                                           if row["mode"] == mode)
                                for mode in ("pruned", "scan")}}
    (args.output / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"latency_ms": args.latency_ms,
                      "trials_per_mode": args.trials,
                      "max_query_ms": summary["max_query_ms"],
                      "counts": [row["count"] for row in observations]}, sort_keys=True))


if __name__ == "__main__":
    main()
