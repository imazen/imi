"""Medians/ranges of check-cold-start.mjs output: first decode vs warm, per arm; pixel-hash agreement."""
import collections
import json
import statistics as st
import sys
from pathlib import Path

d = Path(sys.argv[1])
rows = [json.loads(line) for line in (d / 'trials.jsonl').read_text().splitlines()]
by = collections.defaultdict(list)
for r in rows:
    by[(r['arm'], r['phase'], str(r['workers']))].append(r)
hashes = collections.defaultdict(set)
for r in rows:
    hashes[(r['phase'], str(r['workers']))].add(r['renderSha256'])


def fmt(values):
    return f"{st.median(values):7.1f} [{min(values):.1f}-{max(values):.1f}] n={len(values)}"


print('canvas render hash identical across every arm and repeat, per phase/workers:',
      all(len(v) == 1 for v in hashes.values()), '|', {k: len(v) for k, v in hashes.items() if len(v) > 1} or 'no disagreements')
print('\ndecode wall ms (WASM region decode, union of worker intervals), median [min-max]')
cells = [('page-first-decode', 'auto'), ('workers-first-decode', '1'), ('warm-pan', '1'), ('workers-first-decode', '4'), ('warm-pan', '4'), ('workers-first-decode', '8'), ('warm-pan', '8')]
for arm in ('default', 'liftoff', 'turbofan'):
    print(f'{arm:9s}', ' | '.join(f"{p[:8]}/{w}: {fmt([r['last']['ms'] for r in by[(arm, p, w)]])}" for p, w in cells))
print('\nfull update ms (dispatch to settled), median')
for arm in ('default', 'liftoff', 'turbofan'):
    print(f'{arm:9s}', ' | '.join(f"{p[:8]}/{w}: {st.median([r['update']['elapsedMs'] for r in by[(arm, p, w)]]):.0f}" for p, w in cells))
print('\nstartup ms from module start (median [min-max]): coordinator wasm init, coordinator ready, first tile displayed, first-view worker init')
for arm in ('default', 'liftoff', 'turbofan'):
    s = [r['startup'] for r in rows if r['arm'] == arm and r['phase'] == 'page-first-decode']
    print(f'{arm:9s} wasm {fmt([x["coordinator"]["wasmMs"] for x in s])} | ready {fmt([x["coordinatorReadyMs"] for x in s])} | firstTile {fmt([x["firstTileMs"] for x in s])} | workerInit {fmt([x["firstViewSetup"]["workerInitMs"] for x in s])}')
print('\nratios of medians: first decode / same-arm warm pan (same worker count as the cell)')
for arm in ('default', 'liftoff', 'turbofan'):
    out = []
    for w in ('1', '4', '8'):
        first = st.median([r['last']['ms'] for r in by[(arm, 'workers-first-decode', w)]])
        warm = st.median([r['last']['ms'] for r in by[(arm, 'warm-pan', w)]])
        out.append(f'{w}w {first / warm:.2f}x')
    pf = st.median([r['last']['ms'] for r in by[(arm, 'page-first-decode', 'auto')]])
    w4 = st.median([r['last']['ms'] for r in by[(arm, 'warm-pan', '4')]])
    print(f'{arm:9s} budget-first/warm: {", ".join(out)} | page-first(4 auto workers)/warm4: {pf / w4:.2f}x ({pf:.0f} ms vs {w4:.0f} ms)')
print('\nwarm turbofan/default (median decode ms):', {w: round(st.median([r['last']['ms'] for r in by[('turbofan', 'warm-pan', w)]]) / st.median([r['last']['ms'] for r in by[('default', 'warm-pan', w)]]), 3) for w in ('1', '4', '8')})
