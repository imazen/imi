"""Real-image group/quality I/O experiment; normalized recorded demand, not a new capture.

Inputs and metric program are explicit. Every generated image is decoded by the
production codec. SSIMULACRA2 describes reconstruction; it does not pick a quality
winner or stand in for the project's multi-metric adoption policy.
"""
import argparse
import csv
import hashlib
import itertools
import json
import math
from pathlib import Path
import shutil
import subprocess
import time


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('manifest', type=Path)
    p.add_argument('capture', type=Path)
    p.add_argument('fixture_binary', type=Path)
    p.add_argument('replay_binary', type=Path)
    p.add_argument('metric_binary', type=Path)
    p.add_argument('output', type=Path)
    a = p.parse_args()
    a.output.mkdir(parents=True, exist_ok=False)
    manifest = json.loads(a.manifest.read_text())
    capture = json.loads(a.capture.read_text())
    scenes = {s['id']: s for s in capture['catalog']['datasets']}
    # Fixed before execution: spans pans, zoom changes, reversals and scene return.
    indices = [0, 1, 2, 5, 10, 16, 24, 32, 40, 50, 65, 79]
    if len(capture['views']) <= max(indices):
        raise ValueError('requires the complete recorded 80-camera domain')
    identity = {'manifest': digest(a.manifest), 'capture': digest(a.capture),
                'camera_indices': indices, 'programs': {},
                'scope': 'normalized camera rectangles retargeted to each source; no live CPU/HTTP/visibility claim'}
    for name in ['fixture_binary', 'replay_binary', 'metric_binary']:
        program = getattr(a, name).resolve()
        identity['programs'][name] = {'path': str(program), 'sha256': digest(program)}
        shutil.copy2(program, a.output / name)
    (a.output / 'identity.json').write_text(json.dumps(identity, indent=2) + '\n')
    results = []

    def run(command, stem):
        command = list(map(str, command))
        stem.with_suffix('.command.json').write_text(json.dumps(command) + '\n')
        start = time.monotonic()
        with stem.with_suffix('.log').open('xb') as log:
            result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=False)
        record = {'rc': result.returncode, 'seconds': time.monotonic() - start}
        stem.with_suffix('.result.json').write_text(json.dumps(record) + '\n')
        result.check_returncode()
        return record

    for source in manifest['sources']:
        raw = Path(source['path'])
        w, h = source['width'], source['height']
        if raw.stat().st_size != w * h * 3 or digest(raw) != source['sha256']:
            raise ValueError('source extent/hash')
        folder = a.output / source['id']
        folder.mkdir()
        shutil.copy2(raw, folder / 'source.rgb')
        raw = folder / 'source.rgb'
        metric_command = [str(a.metric_binary.resolve()), 'score', str(raw.resolve()), str(w), str(h)]
        (folder / 'metric-command.json').write_text(json.dumps(metric_command) + '\n')
        with (folder / 'metric.stderr').open('xb') as stderr:
            metric = subprocess.Popen(metric_command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=stderr, text=True, bufsize=1)
            try:
                def score(path):
                    metric.stdin.write(str(path.resolve()) + '\n')
                    metric.stdin.flush()
                    line = metric.stdout.readline()
                    value = float(line)
                    if not math.isfinite(value):
                        raise ValueError('metric result')
                    return value
                identity_score = score(raw)
                if abs(identity_score - 100) > 0.001:
                    raise ValueError('metric identity control')
                (folder / 'metric-control.json').write_text(json.dumps({'score': identity_score}) + '\n')
                for quality, group in itertools.product(['lossless', '5', '20', '50', '90'], [256, 512, 1024, 0]):
                    tw, th = (group, group // 2) if group else (w, h)
                    name = f'q{quality}-g{group}'
                    cell = folder / name
                    encode = run([a.fixture_binary.resolve(), raw.resolve(), w, h, tw, th, quality,
                                  cell.resolve(), 'commit'], folder / (name + '-encode'))
                    reconstruction = cell / 'decoded.rgb'
                    if quality == 'lossless' and digest(raw) != digest(reconstruction):
                        raise ValueError('lossless reconstruction')
                    measured_quality = score(reconstruction)
                    with (cell / 'entries.tsv').open() as stream:
                        entries = list(csv.DictReader(stream, delimiter='\t'))
                    payload = sum(int(e['length']) for e in entries)
                    trace = []
                    demand = []
                    for i in indices:
                        view = capture['views'][i]
                        msg = view['message']
                        original = scenes[msg['scene']]
                        b = msg['bounds']
                        x = max(0, math.floor(b['x'] * w / original['width']))
                        y = max(0, math.floor(b['y'] * h / original['height']))
                        right = min(w, math.ceil((b['x'] + b['w']) * w / original['width']))
                        bottom = min(h, math.ceil((b['y'] + b['h']) * h / original['height']))
                        if right <= x or bottom <= y:
                            raise ValueError('empty normalized camera')
                        keys = [int(e['key']) for e in entries if int(e['x']) < right
                                and int(e['y']) < bottom and int(e['x']) + int(e['width']) > x
                                and int(e['y']) + int(e['height']) > y]
                        if not keys:
                            raise ValueError('no intersecting groups')
                        label = f"camera-{msg['id']}"
                        time_ms = view['at'] - capture['views'][0]['at']
                        row = [label, time_ms, str((cell / ('image.imiset' if group else 'image.imi')).resolve()),
                               'archive' if group else 'image', ','.join(map(str, keys)) if group else '-',
                               *([0, 0, 0, 0] if group else [x, y, right - x, bottom - y])]
                        trace.append('\t'.join(map(str, row)))
                        demand.append({'label': label, 'rect': [x, y, right-x, bottom-y],
                                       'useful_pixels': (right-x)*(bottom-y), 'keys': keys})
                    (cell / 'trace.tsv').write_text('\n'.join(trace) + '\n')
                    (cell / 'demand.json').write_text(json.dumps(demand) + '\n')
                    network = []
                    for mode, cache in [('ranges', 0), ('ranges', 33554432), ('whole', 33554432)]:
                        prefix = cell / f'{mode}-{cache}'
                        run([a.replay_binary.resolve(), cell / 'trace.tsv', prefix, mode, cache,
                             160, 12500000, 8, 'batch', 'shared'], cell / f'{mode}-{cache}-run')
                        with prefix.with_suffix('.views.tsv').open() as stream:
                            views = list(csv.DictReader(stream, delimiter='\t'))
                        if len(views) != len(indices):
                            raise ValueError('native view domain')
                        network.append({'mode': mode, 'cache': cache,
                                        'bytes': sum(int(v['bytes']) for v in views),
                                        'rounds': sum(int(v['dependency_rounds']) for v in views),
                                        'requests': sum(int(v['requests']) for v in views),
                                        'returned_pixels': sum(int(v['decoded_bytes']) // 3 for v in views),
                                        'network_ms': sum(float(v['finish_ms']) - float(v['start_ms']) for v in views)})
                    result = {'source': source['id'], 'width': w, 'height': h, 'quality': quality,
                              'group': [tw, th], 'outer_group': group, 'entries': len(entries),
                              'payload_bytes': payload, 'archive_bytes': (cell / 'image.imiset').stat().st_size,
                              'ssim2': measured_quality, 'encode_job': encode, 'network': network,
                              'source_sha256': source['sha256'], 'reconstruction_sha256': digest(reconstruction)}
                    results.append(result)
                    with (a.output / 'results.jsonl').open('a') as stream:
                        stream.write(json.dumps(result) + '\n')
                    print(source['id'], name, 'PASS', flush=True)
            finally:
                metric.stdin.close()
                code = metric.wait()
                (folder / 'metric.rc').write_text(str(code) + '\n')
                if code:
                    raise RuntimeError('metric process failed')
    (a.output / 'results.json').write_text(json.dumps({'identity': identity, 'results': results}, indent=2) + '\n')
    print(f'{len(results)} actual encoded geometry/rate cells PASS', flush=True)


if __name__ == '__main__':
    main()
