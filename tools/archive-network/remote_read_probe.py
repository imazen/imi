#!/usr/bin/env python3
"""Probe existing immutable remote objects using a frozen HTTP range client.

Configuration names mode-600 URL files on the client host; signed URL contents
are never copied into results. Client checks 206 framing, stable ETag and exact
source pixels. These are client-side R2 counters, not independent server logs.
"""
import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import shlex
import subprocess

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('config',type=Path)
p.add_argument('output',type=Path)
a=p.parse_args()
c=json.loads(a.config.read_text())
a.output.mkdir(exist_ok=False)
(a.output/'CONFIG.json').write_text(json.dumps(c,indent=2)+'\n')
results=[]
s=dict(stage='running',controller_pid=os.getpid(),child_pid=None,completed=0,
       required=len(c['cases'])*len(c['policies'])*c['trials'])
def progress(**kw):
    s.update(kw,checked_utc=datetime.now(timezone.utc).isoformat())
    (a.output/'STATUS.json').write_text(json.dumps(s,indent=2)+'\n')
def totals(value):
    if 'requests' in value:
        return value['requests'], value['body_bytes']
    return value['open_requests'] + value['query_requests'], value['open_body_bytes'] + value['query_body_bytes']
try:
    for trial in range(c['trials']):
        for case in c['cases']:
            for policy in c['policies']:
                assert case['url_file'].startswith('/')
                name=f'{case["name"]}-{policy}-{trial}'
                environment=['env','-u','IMI_HTTP_CACHE'] if policy=='default' else ['env',f'IMI_HTTP_CACHE={policy}']
                command=[*environment,*c['remote_prefix'],c['client'],
                         '@'+case['url_file'],*case['args']]
                child=subprocess.Popen(['ssh',c['remote'],shlex.join(command)],stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
                progress(current=name,child_pid=child.pid)
                stdout,stderr=child.communicate()
                (a.output/(name+'.stdout')).write_text(stdout)
                (a.output/(name+'.stderr')).write_text(stderr)
                assert child.returncode==0,(name,child.returncode)
                value=json.loads(stdout)
                identity=value.get('pixel_xxh3_128',value.get('pixel_sha256',value.get('key_sha256')))
                assert identity
                prior=[x for x in results if x['case']==case['name']]
                assert all(x['pixel_identity']==identity for x in prior)
                same=[x for x in prior if x['policy']==policy]
                assert all(totals(x['result'])==totals(value) for x in same)
                results.append(dict(case=case['name'],policy=policy,trial=trial,pixel_identity=identity,result=value))
                (a.output/'RESULTS.json').write_text(json.dumps(results,indent=2)+'\n')
                progress(completed=len(results),child_pid=None)
                print(name,*totals(value),flush=True)
    progress(stage='complete')
except BaseException as error:
    progress(stage='failed',error=str(error))
    raise
