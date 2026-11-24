#!/usr/bin/env python3
"""Exercise revision resolution and failure propagation without codec builds."""

import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('scratch', type=Path)
args = parser.parse_args()
args.scratch.mkdir(parents=True, exist_ok=True)
root = Path(tempfile.mkdtemp(prefix='byte-gate-runner-', dir=args.scratch))
scripts = root / 'tools' / 'byte-gate'
scripts.mkdir(parents=True)
for name in ['run.sh', 'main.rs', 'grid.rs', 'Cargo.toml.template']:
    shutil.copyfile(Path(__file__).parent / name, scripts / name)
mock = root / 'mock-bin'
mock.mkdir()

def executable(name, source):
    p = mock / name
    p.write_text(source)
    p.chmod(0o755)

executable('jj', '''#!/usr/bin/env python3
import os, sys
if sys.argv[1:3] == ['git', 'root']:
    print(os.environ['MOCK_ROOT'])
elif sys.argv[1] == 'log':
    rev = sys.argv[sys.argv.index('-r') + 1]
    print(('1' if rev == 'main@origin' else '2') * (80 if rev == 'multiple' else 40), end='')
else:
    sys.exit(3)
''')
executable('git', '''#!/usr/bin/env python3
import sys, tarfile
if 'rev-parse' in sys.argv:
    print('3' * 40)  # Git @ deliberately differs from this jj workspace.
elif 'archive' in sys.argv:
    with tarfile.open(fileobj=sys.stdout.buffer, mode='w|'):
        pass
else:
    sys.exit(3)
''')
executable('cargo', '''#!/usr/bin/env python3
import os, pathlib, sys
if os.environ['MOCK_CASE'] == 'compile_failure':
    sys.exit(17)
p = pathlib.Path(os.environ['CARGO_TARGET_DIR']) / 'release' / 'gate'
p.parent.mkdir(parents=True)
p.write_text('#!/bin/sh\\n' + ("printf 'fixture\\t1\\tencoded\\tpixels\\n'\\n"
    if os.environ['MOCK_CASE'] == 'success' else ':\\n'))
p.chmod(0o755)
''')

for case in ['success', 'compile_failure', 'empty_output', 'multiple']:
    work = root / case
    (work / 'raw').mkdir(parents=True)
    (work / 'raw' / 'fixture_1_1.rgb8').write_bytes(bytes([0, 1, 2]))
    env = dict(os.environ, PATH=str(mock) + os.pathsep + os.environ['PATH'],
               MOCK_ROOT=str(root), MOCK_CASE=case)
    run = subprocess.run(['bash', str(scripts / 'run.sh'), 'main@origin',
                          'multiple' if case == 'multiple' else '@', str(work)],
                         env=env, capture_output=True, text=True)
    (work / 'runner.log').write_text(run.stdout + run.stderr)
    if case == 'success':
        assert run.returncode == 0, run.stderr
        assert 'building 222222222222' in run.stderr
        assert 'building 333333333333' not in run.stderr
        assert 'default: identical (1 encodes)' in run.stdout
        assert 'lab: identical (1 encodes)' in run.stdout
    else:
        assert run.returncode == 2, (case, run.returncode, run.stderr)
        assert not list(work.glob('rev-*/*.tsv')), case
        if case == 'empty_output':
            assert 'empty gate output' in run.stderr
        elif case == 'multiple':
            assert 'expected one commit' in run.stderr
    print(case + ': pass')
print('Retained runner evidence: ' + str(root))
