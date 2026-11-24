#!/usr/bin/env python3
"""Stage the bounded synthetic producer test as a local viewer catalog.

The existing IMIRNG01/IMITILE1 browser sidecars require their legacy 32-byte
identities. This fixture preserves that protocol; V4 object/tile hashes are XXH3.
No remote upload, source download or metric maps are involved.
"""
from pathlib import Path
import argparse
import hashlib
import json
import shutil

p=argparse.ArgumentParser()
p.add_argument('prepared',type=Path,help='complete-raw output from the producer test')
p.add_argument('output',type=Path,help='fresh local fixture root')
a=p.parse_args();a.output.mkdir(parents=True,exist_ok=False)
shutil.copytree(a.prepared,a.output/'data')
shutil.copyfile(a.prepared.parent/'source.rgb',a.output/'data/source.rgb')
def asset(path):
    body=(a.output/'data'/path).read_bytes()
    return {'url':'/image/'+str(path),'sha256':hashlib.sha256(body).hexdigest(),'encodedBytes':len(body)}
levels=[{'key':'full','divisor':1}]
reps={'lossless':{},'lossy':{}}
for key,divisor,width,height,base in [('full',1,1033,577,Path('.')),('half',2,517,289,Path('pyramid/level-2')),('quarter',4,259,145,Path('pyramid/level-4'))]:
    if key!='full':levels.append({'key':key,'divisor':divisor})
    full=key=='full'
    loss=asset(base/('full-lossless.imi' if full else 'lossless.imi'))
    boot=asset(base/('full-lossless-cache.bin' if full else 'lossless-cache.bin'))
    loss.update(width=width,height=height,divisor=divisor,kind='imi',bootstrap={'url':boot['url'],'sha256':boot['sha256'],'bytes':boot['encodedBytes']})
    archive=asset(base/('full-lossy.imijset' if full else 'lossy.imijset'));index=asset(base/('full-lossy.bin' if full else 'lossy.bin'))
    archive.update(width=width,height=height,divisor=divisor,kind='tiles',qualityTarget=80,effort=10,index={'url':index['url'],'sha256':index['sha256'],'bytes':index['encodedBytes']})
    reps['lossless'][key]=loss;reps['lossy'][key]=archive
scene={'id':'v4-pattern','title':'V4 synthetic clipped groups','width':1033,'height':577,'sourceUrl':'https://github.com/imazen/zqi','credit':'Synthetic codec integration fixture','levels':levels,'representations':reps}
catalog={'datasets':[scene]}
for name in ['image.json','gallery.json']:(a.output/'data'/name).write_text(json.dumps(catalog,indent=2)+'\n')
print(a.output/'data/image.json')
