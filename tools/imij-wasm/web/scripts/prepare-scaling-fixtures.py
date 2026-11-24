"""Real RGB8 controls for worker scaling; not a quality/rate comparison."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time

p = argparse.ArgumentParser()
p.add_argument('binary', type=Path)
p.add_argument('raw', type=Path)
p.add_argument('width', type=int)
p.add_argument('height', type=int)
p.add_argument('output', type=Path)
p.add_argument('--modes', nargs='+', choices=['lossy-runlevel', 'lossy-splithuf', 'lossy-fastest', 'quick'], default=['lossy-runlevel', 'lossy-splithuf', 'lossy-fastest', 'quick'])
a = p.parse_args()
a.output.mkdir(exist_ok=False)
binary = a.binary.resolve()

def sha(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

if a.raw.stat().st_size != a.width * a.height * 3:
    raise ValueError('RGB8 extent')
commands = []
fixtures = []
(a.output / 'inputs.json').write_text(json.dumps({'binary': str(binary), 'binarySha256': sha(binary), 'raw': str(a.raw), 'rawSha256': sha(a.raw)}, indent=2))
for mode, config in [
    ('lossy-runlevel', ['--lossy', '--effort', '10', '--quality', '81.25', '--entropy', 'runlevel']),
    ('lossy-splithuf', ['--lossy', '--effort', '10', '--quality', '81.25', '--entropy', 'splithuf']),
    ('lossy-fastest', ['--lossy', '--effort', '10', '--quality', '81.25', '--decode-speed', 'fastest']),
    ('quick', ['--quick']),
]:
    if mode not in a.modes:
        continue
    encoded = a.output / (mode + '.imi')
    raw = a.output / (mode + '.rgb')
    for name, args, stdin, stdout in [
        ('encode', ['encode', '--raw', str(a.width), str(a.height), 'rgb', 'u8', *config], a.raw, encoded),
        ('decode', ['decode-raw'], encoded, raw),
        ('trace', ['trace'], encoded, a.output / (mode + '.trace.json')),
    ]:
        cmd = [str(binary), *args]
        record = {'mode': mode, 'operation': name, 'command': cmd, 'input': str(stdin), 'output': str(stdout)}
        (a.output / (mode + '-' + name + '.command.json')).write_text(json.dumps(record, indent=2))
        start = time.monotonic()
        with stdin.open('rb') as source, stdout.open('xb') as target, (a.output / (mode + '-' + name + '.log')).open('xb') as log:
            result = subprocess.run(cmd, stdin=source, stdout=target, stderr=log)
        record.update(rc=result.returncode, seconds=time.monotonic() - start)
        commands.append(record)
        (a.output / 'commands.json').write_text(json.dumps(commands, indent=2))
        result.check_returncode()
    if raw.stat().st_size != a.width * a.height * 3:
        raise ValueError('native RGB output extent')
    if mode == 'quick' and sha(raw) != sha(a.raw):
        raise ValueError('Quick source pixels')
    fixtures.append({'id': mode, 'file': encoded.name, 'bytes': encoded.stat().st_size, 'sha256': sha(encoded), 'decSha': sha(raw), 'width': a.width, 'height': a.height})
(a.output / 'fixtures.json').write_text(json.dumps(fixtures, indent=2))
