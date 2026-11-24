"""Reconcile every completed replay with physical ranges and its recorded policy."""
import argparse
import csv
import json
import math
from collections import defaultdict
from pathlib import Path

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('comparison', type=Path)
a = p.parse_args()
root = json.loads((a.comparison / 'summary.json').read_text())
cases = 0
requests = 0
for directory in [a.comparison / 'original', *sorted(a.comparison.glob('*-matrix'))]:
    summary = json.loads((directory / 'summary.json').read_text())
    if summary['pixel_domains'] != root['pixel_domains']:
        raise ValueError('comparison pixel domains differ')
    for cell in summary['results']:
        name = f"{cell['delivery']}-{cell['mode']}-cache{cell['cache_bytes']}-rtt{cell['rtt_ms']}-n{cell['concurrency']}"
        with (directory / (name + '.views.tsv')).open() as f:
            views = list(csv.DictReader(f, delimiter='\t'))
        with (directory / (name + '.ranges.tsv')).open() as f:
            events = list(csv.DictReader(f, delimiter='\t'))
        per_view = defaultdict(list)
        for event in events:
            start, end, length = (int(event[k]) for k in ['start', 'end_exclusive', 'object_length'])
            logical_start, logical_end = (int(event[k]) for k in ['logical_start', 'logical_end'])
            if not 0 <= start < end <= length or logical_end - logical_start != end - start:
                raise ValueError('physical/logical range bounds')
            if event['status'] != ('206' if cell['mode'] == 'ranges' else '200'):
                raise ValueError('response status')
            if cell['mode'] == 'whole' and (start != 0 or end != length):
                raise ValueError('non-range response was not a whole physical object')
            per_view[event['label']].append(event)
        for view in views:
            selected = per_view.pop(view['label'], [])
            if len(selected) != int(view['requests']) or sum(int(e['end_exclusive']) - int(e['start']) for e in selected) != int(view['bytes']):
                raise ValueError('view request/byte accounting')
            waves = defaultdict(list)
            for e in selected:
                waves[(e['round'], e['start_ms'], e['finish_ms'])].append(e)
            time = 0.0
            for (_, start, end), wave in waves.items():
                if len(wave) > cell['concurrency']:
                    raise ValueError('physical request concurrency')
                expected = cell['rtt_ms'] + sum(int(e['end_exclusive']) - int(e['start']) for e in wave) * 1000 / cell['bytes_per_second']
                if not math.isclose(float(end) - float(start), expected, rel_tol=1e-9, abs_tol=1e-8):
                    raise ValueError('wave RTT/shared-bandwidth cost')
                time += expected
            if not math.isclose(float(view['finish_ms']) - float(view['start_ms']), time, rel_tol=1e-9, abs_tol=1e-8):
                raise ValueError('view network time')
        if per_view or len(events) != cell['requests'] or sum(int(e['end_exclusive']) - int(e['start']) for e in events) != cell['transferred_bytes']:
            raise ValueError('summary accounting')
        cases += 1
        requests += len(events)
if cases != len(root['original']) + len(root['results']):
    raise ValueError('missing or extra scenario')
print(json.dumps({'scenarios': cases, 'physical_requests': requests, 'all_ranges_costs_pixels_reconciled': True}))
