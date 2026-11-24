#!/usr/bin/env python3
"""Replay immutable archive reads over LAN HTTP with explicit link controls.

CONFIG holds archive paths and exact existing-client argv tails. A fresh remote
process runs each case. Source/binary provenance uses XXH3; the reused client's
legacy correctness fields are retained unchanged. No image data is published.
"""
import argparse
import json
import os
from datetime import datetime, timezone
from pathlib import Path
import re
import shlex
import subprocess
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import xxhash


def identity(path):
    with Path(path).open('rb') as stream:
        value = xxhash.xxh3_128()
        while block := stream.read(1024 * 1024):
            value.update(block)
        return value.hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('config', type=Path)
    p.add_argument('output', type=Path)
    a = p.parse_args()
    config = json.loads(a.config.read_text())
    a.output.mkdir(exist_ok=False, parents=True)
    status = dict(stage='preparing', controller_pid=os.getpid(), child_pid=None,
                  completed=0, required=len(config['profiles'])*len(config['cases'])*config.get('trials', 3))
    def progress(**values):
        status.update(values, checked_utc=datetime.now(timezone.utc).isoformat())
        (a.output/'STATUS.json').write_text(json.dumps(status, indent=2)+'\n')
    progress()
    files = {name: Path(path) for name, path in config['archives'].items()}
    ids = {name: identity(path) for name, path in files.items()}
    trace = []
    lock = threading.Lock()
    policy = {'delay_ms': 0, 'bytes_per_second': 0}

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_GET(self):
            path = files.get(self.path.removeprefix('/'))
            match = re.fullmatch(r'bytes=(\d+)-(\d+)', self.headers.get('Range', ''))
            if path is None or match is None:
                self.send_error(400)
                return
            start, end = map(int, match.groups())
            if not 0 <= start <= end < path.stat().st_size:
                self.send_error(416)
                return
            etag = '"' + ids[self.path[1:]] + '"'
            if self.headers.get('If-Match', etag) != etag:
                self.send_error(412)
                return
            time.sleep(policy['delay_ms'] / 1000)
            self.send_response(206)
            self.send_header('Content-Range', f'bytes {start}-{end}/{path.stat().st_size}')
            self.send_header('Content-Length', str(end-start+1))
            self.send_header('ETag', etag)
            self.end_headers()
            with path.open('rb') as stream:
                stream.seek(start)
                left = end-start+1
                while left:
                    block = stream.read(min(left, 64*1024))
                    if not block:
                        raise RuntimeError('immutable input truncated')
                    if policy['bytes_per_second']:
                        time.sleep(len(block)/policy['bytes_per_second'])
                    self.wfile.write(block)
                    left -= len(block)
            with lock:
                trace.append(dict(object=self.path[1:], start=start, end=end,
                                  bytes=end-start+1, peer=self.client_address[0],
                                  if_match=self.headers.get('If-Match')))

    server = ThreadingHTTPServer(('0.0.0.0', config['port']), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    results = []
    try:
        for profile in config['profiles']:
            policy.update(profile)
            for case in config['cases']:
                for trial in range(config.get('trials', 3)):
                    with lock:
                        before = len(trace)
                    url = f'http://{config["advertise"]}:{server.server_port}/{case["archive"]}'
                    argv = [*config['remote_prefix'], config['client'], url, *case['args']]
                    name = f'{profile["name"]}-{case["name"]}-{trial}'
                    child = subprocess.Popen(['ssh', config['remote'], shlex.join(argv)],
                                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                    progress(stage='running', current=name, child_pid=child.pid)
                    stdout, stderr = child.communicate()
                    progress(child_pid=None)
                    (a.output/(name+'.stdout')).write_text(stdout)
                    (a.output/(name+'.stderr')).write_text(stderr)
                    assert child.returncode == 0, (name, stderr)
                    row = json.loads(stdout)
                    with lock:
                        requests = trace[before:].copy()
                    count = row.get('requests', sum(row.get(k+'_requests', 0) for k in ('open','lookup','copy','query')))
                    size = row.get('body_bytes', sum(row.get(k+'_body_bytes', 0) for k in ('open','lookup','copy','query')))
                    assert len(requests) == count and sum(r['bytes'] for r in requests) == size
                    assert all(r['peer'] != '127.0.0.1' for r in requests)
                    assert all(r['if_match'] == '"'+ids[r['object']]+'"' for r in requests[1:])
                    results.append(dict(name=name, profile=profile, case=case['name'],
                                        result=row, requests=requests))
                    (a.output/'RESULTS.json').write_text(json.dumps(results, indent=2)+'\n')
                    progress(completed=len(results))
                    print(name, count, size, flush=True)
    except BaseException as error:
        progress(stage='failed', error=str(error))
        raise
    finally:
        server.shutdown()
        thread.join()
        server.server_close()
    assert {name: identity(path) for name, path in files.items()} == ids
    (a.output/'IDENTITY.json').write_text(json.dumps(dict(config=config, archives_xxh3=ids,
        scope='Separate-host LAN; delay is per-response and bandwidth is application-paced. Not packet loss, TCP RTT emulation, WAN or CPU throughput qualification.'), indent=2)+'\n')
    progress(stage='complete')


if __name__ == '__main__':
    main()
