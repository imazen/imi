#!/usr/bin/env python3
"""Prepare exact ROI oracles from retained original RGB8 samples, without maps."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('helper', type=Path)
p.add_argument('source_table', type=Path)
p.add_argument('iqa_raw', type=Path)
p.add_argument('retained_inputs', type=Path)
p.add_argument('output', type=Path)
a=p.parse_args()
a.output.mkdir(exist_ok=False)
def identity(path):
    return subprocess.check_output([str(a.helper),'xxh3',str(path)], text=True).strip()
source_rows=[line.split('\t') for line in a.source_table.read_text().splitlines()]
sources=[]
for ordinal in [63,125]:
    path=a.iqa_raw/f'{ordinal}.raw'
    data=path.read_bytes()
    # Original sealed external-oracle identity; new identities below use XXH3.
    assert hashlib.sha256(data).hexdigest()==source_rows[ordinal][4]
    sources.append((str(ordinal),path,512,384))
retained=json.loads((a.retained_inputs/'RAW_INPUTS.json').read_text())
entry=next(v for v in retained if v['case']=='1415-4096x4096')
path=a.retained_inputs/entry['archive_path']
assert identity(path)==entry['xxh3_128']
assert entry['descriptor']['bits']==8 and entry['descriptor']['channels']==3
sources.append(('4k',path,4096,4096))
cases=[]
for name,path,width,height in sources:
    data=path.read_bytes();assert len(data)==width*height*3
    rectangles=[('center',width//2-32,height//2-32,64,64),('edge',width-127,height-65,127,65)]
    if name=='4k':
        rectangles += [('viewport',1536,1536,1024,1024),('full',0,0,width,height)]
    for label,x,y,w,h in rectangles:
        pixels=b''.join(data[((y+row)*width+x)*3:((y+row)*width+x+w)*3] for row in range(h))
        output=path if label=='full' else a.output/f'{name}-{label}.rgb'
        if label!='full':
            with output.open('xb') as f:f.write(pixels)
        cases.append(dict(source=name,key=1 if name=='4k' else int(name)*2+1,rect=[x,y,w,h],
                          expected=str(output.resolve()),xxh3_128=identity(output)))
(a.output/'CASES.json').write_text(json.dumps(cases,indent=2)+'\n')
print(json.dumps(dict(sources=len(sources),roi_cases=len(cases))))
