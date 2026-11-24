"""Freeze captured viewer demands against SHA-verified native archive files.

No encoding or pixel resampling. Output both all-dispatched and actually-settled
traces; the latter excludes superseded views, using observed completion IDs.
"""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path
from urllib.parse import urljoin
from urllib.request import Request, urlopen


def prepare(trace, destination, max_bytes, layout_binary=None):
    if trace.get('schema') != 1 or not trace['views']:
        raise ValueError('unsupported/empty viewer trace')
    records = []
    objects = {}
    for view in trace['views']:
        m = view['message']
        scene = next(s for s in trace['catalog']['datasets'] if s['id'] == m['scene'])
        rep = scene['representations'][m['quality']][m['level']]
        if rep['kind'] != 'tiles':
            raise ValueError('this trace adapter requires row-major native tile archives')
        if rep.get('parts') and layout_binary is None:
            raise ValueError('segmented hydration requires --layout-binary for verified physical part reconstruction')
        sha = rep['sha256']
        if len(sha) != 64 or any(c not in '0123456789abcdef' for c in sha):
            raise ValueError('invalid object digest')
        objects[sha] = rep
        # nasa_pyramid assigns row-major keys; native get_many must locate them.
        keys = set()
        for t in m['tiles']:
            if not t['visible']:
                continue
            for y in range(t['ly'] // 256, (t['ly'] + t['lh'] + 255) // 256):
                for x in range(t['lx'] // 512, (t['lx'] + t['lw'] + 511) // 512):
                    keys.add(y * ((rep['width'] + 511) // 512) + x)
        records.append((view, sha, sorted(keys)))
    total = sum(r['encodedBytes'] for r in objects.values())
    if total > max_bytes:
        raise ValueError(f'{total} source bytes exceed explicit budget {max_bytes}')
    destination.mkdir(exist_ok=False, parents=True)
    inventory = []
    paths = {}
    for sha, rep in objects.items():
        url = urljoin(trace['url'], rep['url'])
        path = destination / (sha + '.imiset')
        digest = hashlib.sha256()
        size = 0
        parts = rep.get('parts')
        if parts:
            part_size = parts['size']
            if not isinstance(part_size, int) or part_size <= 0 or parts['count'] != (rep['encodedBytes'] + part_size - 1) // part_size:
                raise ValueError('part geometry')
            base = urljoin(trace['url'], parts['base']).rstrip('/') + '/'
            urls = [(urljoin(base, f'{i:06}.bin'), min(part_size, rep['encodedBytes'] - i * part_size)) for i in range(parts['count'])]
        else:
            urls = [(url, rep['encodedBytes'])]
        physical = []
        with path.open('xb') as output:
            for part_url, expected in urls:
                part_hash = hashlib.sha256()
                part_bytes = 0
                with urlopen(Request(part_url, headers={"User-Agent": "imi-access-model/1"}), timeout=30) as response:
                    while chunk := response.read(1024 * 1024):
                        part_bytes += len(chunk)
                        if part_bytes > expected:
                            raise ValueError('source part exceeded declared size')
                        size += len(chunk)
                        output.write(chunk)
                        digest.update(chunk)
                        part_hash.update(chunk)
                if part_bytes != expected:
                    raise ValueError('truncated source part')
                physical.append({'url': part_url, 'bytes': part_bytes, 'sha256': part_hash.hexdigest()})
        if size != rep['encodedBytes'] or digest.hexdigest() != sha:
            raise ValueError(f'source identity mismatch: {url}')
        replay_path = path
        if parts:
            command = [str(Path(layout_binary).resolve()), 'split', str(path.resolve()), str(parts['size'])]
            (destination / (sha + '.split-command.json')).write_text(json.dumps(command) + '\n')
            with (destination / (sha + '.split.log')).open('x') as log:
                result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, check=False)
            (destination / (sha + '.split.rc')).write_text(str(result.returncode) + '\n')
            if result.returncode:
                raise RuntimeError('part reconstruction failed; evidence retained')
            replay_path = path.with_name(path.name + '.parts.tsv')
            if not replay_path.is_file():
                raise ValueError('missing part manifest')
        paths[sha] = replay_path.resolve()
        inventory.append({'path': str(path.resolve()), 'replay_path': str(replay_path.resolve()),
                          'url': url, 'sha256': sha, 'bytes': size, 'physical_sources': physical})
    settled = {r['id'] for r in trace['settled']}
    origin = records[0][0]['at']
    for name, select in [('all', lambda m: True), ('settled', lambda m: m['id'] in settled)]:
        with (destination / f'{name}.tsv').open('x') as output:
            output.write('# label\ttime_ms\tfile\tkind\tkeys\tx\ty\twidth\theight\n')
            for view, sha, keys in records:
                m = view['message']
                if select(m):
                    label = f"{m['id']}:{m['scene']}:{m['quality']}:{m['level']}"
                    if any(c in label for c in '\t\r\n'):
                        raise ValueError('invalid label')
                    fields = [label, str(view['at'] - origin), str(paths[sha]),
                              'archive', ','.join(map(str, keys)) or '-', '0', '0', '0', '0']
                    output.write('\t'.join(fields) + '\n')
    (destination / 'inventory.json').write_text(json.dumps(inventory, indent=2) + '\n')
    print(f'{len(records)} dispatched demands, {len(settled)} settled IDs, {len(objects)} verified objects, {total} bytes')


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('trace', type=Path)
    p.add_argument('output', type=Path)
    p.add_argument('--max-bytes', required=True, type=int)
    p.add_argument('--layout-binary', type=Path)
    args = p.parse_args()
    prepare(json.loads(args.trace.read_text()), args.output, args.max_bytes, args.layout_binary)
