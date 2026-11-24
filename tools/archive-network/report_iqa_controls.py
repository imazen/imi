#!/usr/bin/env python3
"""Compare complete matched IQA controls without hiding slower observations."""
import argparse
import json
from pathlib import Path
import statistics

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('root', type=Path)
p.add_argument('output', type=Path)
a = p.parse_args()
assert not a.output.exists()
status = json.loads((a.root/'STATUS.json').read_text())
assert status['stage'] == 'complete' and status['completed'] == status['required']
data = json.loads((a.root/'RESULTS.json').read_text())
config_file = a.root/'CONFIG.json'
config = json.loads(config_file.read_text()) if config_file.exists() else json.loads((a.root/'IDENTITY.json').read_text())['config']
profiles = config.get('profiles', [dict(name=p) for p in config.get('policies', [])])
assert len(data) == len(config['cases'])*len(profiles)*config['trials']
observations = [(r['case'], r.get('policy', r.get('profile', {}).get('name')),
                 r.get('trial', r.get('name'))) for r in data]
assert all(row[2] is not None for row in observations)
assert len(set(observations)) == len(data)
names = [case['name'] for case in config['cases']]
pairs = []
if all('pair' in case and 'version' in case for case in config['cases']):
    grouped = {}
    for case in config['cases']:
        grouped.setdefault(case['pair'], {})[case['version']] = case['name']
    assert all(sorted(pair) == [3, 4] for pair in grouped.values())
    pairs = [(pair[3], pair[4]) for pair in grouped.values()]
else:
    for name in names:
        if name.startswith('v3-'):
            pairs.append((name, 'v4-'+name[3:]))
        elif name.startswith('independent-summary-'):
            pairs.append((name, name.replace('independent-', 'independent-v4-', 1)))
        elif name.startswith('reference-v3-reindexed-summary-'):
            pairs.append((name, name.replace('reference-v3-reindexed-', 'reference-v4-', 1)))
assert pairs and all(right in names for _, right in pairs)
assert len(pairs)*2 == len(names)

def reduce(values):
    assert len(values) == config['trials']
    result = []
    identities = set()
    for row in values:
        v = row['result']
        identities.add(v.get('pixel_xxh3_128', v.get('pixel_sha256', v.get('key_sha256'))))
        result.append(dict(requests=v.get('requests', v.get('open_requests', 0)+v.get('query_requests', 0)),
                           body_bytes=v.get('body_bytes', v.get('open_body_bytes', 0)+v.get('query_body_bytes', 0)),
                           wall_ms=v.get('wall_ms', v.get('open_ms', 0)+v.get('query_ms', 0)),
                           operations=v.get('operations')))
    assert len(identities) == 1 and None not in identities
    assert len({(v['requests'],v['body_bytes']) for v in result}) == 1
    return dict(identity=identities.pop(), requests=result[0]['requests'], body_bytes=result[0]['body_bytes'],
                median_ms=statistics.median(v['wall_ms'] for v in result), trials=result)

rows = []
for profile in profiles:
    for left, right in pairs:
        before, after = [reduce([r for r in data if r['case']==name and r.get('policy', r.get('profile', {}).get('name'))==profile['name']]) for name in [left,right]]
        assert before['identity'] == after['identity']
        rows.append(dict(profile=profile['name'], case=left[3:] if left.startswith('v3-') else left,
                         before=before, after=after, extra_requests=after['requests']-before['requests'],
                         extra_bytes=after['body_bytes']-before['body_bytes'], time_ratio=after['median_ms']/before['median_ms']))
summary = dict(comparisons=len(rows), request_worse=sum(r['extra_requests']>0 for r in rows),
               bytes_worse=sum(r['extra_bytes']>0 for r in rows),
               slower_median_observed=sum(r['time_ratio']>1 for r in rows),
               qualification='Observed medians lack paired A/A/noise qualification; this is not a universal no-regression certificate.')
a.output.write_text(json.dumps(dict(summary=summary, rows=rows), indent=2)+'\n')
print(json.dumps(summary))
