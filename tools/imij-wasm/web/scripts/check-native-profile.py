#!/usr/bin/env python3
"""Native decode ceiling on the browser's frozen RGB inputs; caller owns resource locks."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import shutil
import subprocess

p = argparse.ArgumentParser()
p.add_argument('binary', type=Path)
p.add_argument('bench', type=Path)
p.add_argument('manifest', type=Path)
p.add_argument('output', type=Path)
p.add_argument('--counts', default='1,4,8')
p.add_argument('--reps', type=int, default=20)
a = p.parse_args()
counts = [int(n) for n in a.counts.split(',')]
if not counts or len(set(counts)) != len(counts) or any(n not in [1, 2, 4, 8, 16] for n in counts) or a.reps < 1:
    raise ValueError('positive repetitions and distinct supported thread counts required')
a.output.mkdir()
a.binary = a.binary.resolve()
program_hash = hashlib.sha256(a.binary.read_bytes()).hexdigest()
shutil.copyfile(a.binary, a.output / 'native-program')
rows = []
for fixture in json.loads(a.manifest.read_text()):
    file = (a.bench / fixture['file']).resolve()
    data = file.read_bytes()
    if hashlib.sha256(data).hexdigest() != fixture['sha256']:
        raise ValueError(f'input hash: {file}')
    for mode, threads in [('pixels', None)] + [('profile', n) for n in counts]:
        command = [str(a.binary), 'decode-raw'] if mode == 'pixels' else [str(a.binary), 'profile-decode', str(threads), str(a.reps), '3']
        identity = {'fixture': fixture['id'], 'mode': mode, 'threads': threads, 'command': command, 'inputSha256': fixture['sha256'], 'programSha256': program_hash}
        with (a.output / 'commands.jsonl').open('a') as f:
            f.write(json.dumps(identity) + '\n')
        result = subprocess.run(command, input=data, capture_output=True)
        prefix = a.output / f"{fixture['id']}-{mode}-{threads}"
        prefix.with_suffix('.stderr').write_bytes(result.stderr)
        prefix.with_suffix('.status.json').write_text(json.dumps({'returncode': result.returncode}))
        if result.returncode:
            prefix.with_suffix('.stdout').write_bytes(result.stdout)
            raise RuntimeError(f'{command}: {result.returncode}')
        if mode == 'pixels':
            if len(result.stdout) != fixture['width'] * fixture['height'] * 3 or hashlib.sha256(result.stdout).hexdigest() != fixture['decSha']:
                prefix.with_suffix('.raw').write_bytes(result.stdout)
                raise ValueError('native RGB reference mismatch')
            prefix.with_suffix('.pixels.json').write_text(json.dumps({'sha256': fixture['decSha'], 'bytes': len(result.stdout)}))
        else:
            prefix.with_suffix('.json').write_bytes(result.stdout)
            observation = json.loads(result.stdout)
            if observation['threads'] != threads or observation['reps'] != a.reps or observation['warmups'] != 3 or not observation['tier'] or len(observation['ms']) != a.reps or any(not math.isfinite(n) or n <= 0 for n in observation['ms']):
                raise ValueError('incomplete or invalid native profile')
            rows.append({**identity, **observation})
if hashlib.sha256(a.binary.read_bytes()).hexdigest() != program_hash:
    raise ValueError('program changed during measurement')
(a.output / 'results.json').write_text(json.dumps({'scope': 'caller-owned output; excludes browser binding/transfer work', 'rows': rows}, indent=2))
print(f'{len(rows)} native timing cells; every input and native RGB reference verified')
