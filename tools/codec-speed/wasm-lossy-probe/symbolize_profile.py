#!/usr/bin/env python3
"""Symbolize a CDP Profiler.stop result from a WASM decode profile.

Reads <profile>.json, resolves wasm frames to names via the function-name
section table (profile_names.json) or by 'wasm-function[<i>]' indices, and
prints self-sample and total-sample rankings. Wasm frame identity is taken
from callFrame.functionName ('wasm-function[N]' or a real name when the
module has a name section) falling back to url/column byte-offset mapping
through the code section.

usage: symbolize_profile.py PROFILE.json NAMES.json [WASM_PATH] [--top N]
"""
import json
import re
import sys


def uleb(d, i):
    r = 0
    sh = 0
    while True:
        b = d[i]
        i += 1
        r |= (b & 0x7F) << sh
        sh += 7
        if not b & 0x80:
            break
    return r, i


def code_offsets(path):
    """Return [(body_start_file_offset, end_offset)] per defined function."""
    data = open(path, 'rb').read()
    i = 8
    bodies = []
    while i < len(data):
        sec = data[i]
        i += 1
        size, i = uleb(data, i)
        if sec == 10:
            n, j = uleb(data, i)
            for _ in range(n):
                bs, j = uleb(data, j)
                bodies.append((j, j + bs))
                j += bs
            break
        i += size
    return bodies


def main():
    profile = json.load(open(sys.argv[1]))
    names = {int(k): v for k, v in json.load(open(sys.argv[2])).items()}
    wasm = sys.argv[3] if len(sys.argv) > 3 and not sys.argv[3].startswith('-') else None
    top = 40
    if '--top' in sys.argv:
        top = int(sys.argv[sys.argv.index('--top') + 1])

    bodies = code_offsets(wasm) if wasm else None
    n_imported = 16  # measured: both modules import 16 functions
    func_re = re.compile(r'wasm-function\[(\d+)\]')

    nodes = {n['id']: n for n in profile['nodes']}
    total_samples = sum(n.get('hitCount', 0) for n in profile['nodes'])
    self_hits = {}
    total_hits = {}

    def label(n):
        cf = n['callFrame']
        fn = cf.get('functionName', '')
        url = cf.get('url', '')
        m = func_re.search(fn)
        if m:
            idx = int(m.group(1))
            return names.get(idx, fn)
        if url.startswith('wasm://'):
            # unnamed wasm frame; try column byte-offset -> body
            off = cf.get('columnNumber', -1)
            if bodies and off >= 0:
                for i, (s, e) in enumerate(bodies):
                    if s <= off < e:
                        idx = n_imported + i
                        return names.get(idx, f'wasm[{idx}]@{off:#x}')
            return f'{fn or "?"}({url.split("/")[-1]}:{cf.get("lineNumber")},{off})'
        return fn or '(anonymous)'

    # self + total (subtree) hits per label
    subtree_sum = {}

    def accumulate(nid):
        n = nodes[nid]
        s = sum(accumulate(c) for c in n.get('children', []))
        own = n.get('hitCount', 0)
        subtree_sum[nid] = s + own
        return s + own

    roots = [n['id'] for n in profile['nodes'] if not any(n['id'] in p.get('children', []) for p in profile['nodes'])]
    for r in roots:
        accumulate(r)
    for nid, n in nodes.items():
        l = label(n)
        self_hits[l] = self_hits.get(l, 0) + n.get('hitCount', 0)
        total_hits[l] = total_hits.get(l, 0) + subtree_sum[nid]

    print(f'# {sys.argv[1]}  fixture={profile.get("__fixture")} samples={total_samples} interval={profile.get("samplingInterval")}us')
    print(f'# pins: {profile.get("__pins")}')
    print('## self-time')
    for l, h in sorted(self_hits.items(), key=lambda kv: -kv[1])[:top]:
        print(f'{h:>8} {100*h/total_samples:6.2f}%  {l[:130]}')
    print('## total-time (function on stack)')
    for l, h in sorted(total_hits.items(), key=lambda kv: -kv[1])[:top]:
        print(f'{h:>8} {100*h/total_samples:6.2f}%  {l[:130]}')


if __name__ == '__main__':
    main()
