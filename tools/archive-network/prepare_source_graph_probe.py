#!/usr/bin/env python3
"""Prepare a disposable exported source tree for the paired PR208 compile gate.

Never edits a checkout or its production pins. Keep the original lockfile and
write the exact manifest-only diff before Cargo resolves the review graph.
"""
import argparse
import json
from pathlib import Path
import shutil
import tomllib

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('exported_source', type=Path)
p.add_argument('fresh_probe', type=Path)
a = p.parse_args()
assert not (a.exported_source / '.git').exists() and not (a.exported_source / '.jj').exists()
shutil.copytree(a.exported_source, a.fresh_probe)
old_zc = '119f31336e3d9a32aa7d78290cfc17035b78f050'
new_zc = 'f4ef1c34b2c5b70a4e4fa4147022f5e32b1931ec'
old_px = '3286169c7ef5b1e66ac841c3d8204eb54e390f8c'
new_px = '76005fc99a7c822af1801a1f9f87e7d3dc98c510'
changed = {}
for name in ['Cargo.toml', 'imij/Cargo.toml']:
    file = a.fresh_probe / name
    before = file.read_text()
    assert old_zc in before and old_px in before
    after = before.replace(old_zc, new_zc).replace(old_px, new_px)
    if name == 'Cargo.toml':
        after += '\n# Exact paired review cohort; this disposable export is not production.\n'
        after += 'garb = { git = "https://github.com/imazen/garb", rev = "58ce9cad7b337418a7abe9b1f6e3e4bd1928b385" }\n'
        after += 'linear-srgb = { git = "https://github.com/imazen/linear-srgb", rev = "52896f4b24664315586bd60e5f048253b1f69052" }\n'
        after += f'zenpixels-convert = {{ git = "https://github.com/imazen/zenpixels", rev = "{new_px}" }}\n'
    tomllib.loads(after)
    file.write_text(after)
    changed[name] = dict(before=before, after=after)
lock = a.fresh_probe / 'Cargo.lock'
lock.rename(a.fresh_probe / 'Cargo.lock.baseline')
file = a.fresh_probe / 'imij-set/Cargo.toml'
before = file.read_text()
after = before + f'\nzencodec = {{ git = "https://github.com/imazen/zencodec", rev = "{new_zc}", default-features = false }}\n'
after += f'zencodec-io = {{ git = "https://github.com/imazen/zencodec", rev = "{new_zc}", default-features = false, features = ["std"] }}\n'
tomllib.loads(after)
file.write_text(after)
changed['imij-set/Cargo.toml'] = dict(before=before, after=after)
shutil.copyfile(Path(__file__).with_name('shared_source_probe.rs'),
                a.fresh_probe / 'imij-set/examples/shared_source_probe.rs')
(a.fresh_probe / 'SOURCE-GRAPH-PROBE.json').write_text(json.dumps(dict(
    scope='Paired graph and private source bridge qualification; no production pin/API change.',
    zencodec=new_zc, zenpixels=new_px, manifests=changed), indent=2) + '\n')
print(json.dumps(dict(prepared=str(a.fresh_probe), zencodec=new_zc, zenpixels=new_px)))
