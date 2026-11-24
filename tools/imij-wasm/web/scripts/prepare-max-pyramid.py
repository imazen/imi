"""Encode every existing scene/level at IMI effort10 from preserved original RGB8 rasters."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import time

p=argparse.ArgumentParser()
p.add_argument('binary',type=Path)
p.add_argument('catalog',type=Path)
p.add_argument('inputs',type=Path)
p.add_argument('output',type=Path)
p.add_argument('--input-prefix',default='')
p.add_argument('--strip-prefix',default='')
a=p.parse_args()
a.binary=a.binary.resolve()
a.output.mkdir(exist_ok=False)
catalog=json.loads(a.catalog.read_text())
inputs=json.loads(a.inputs.read_text())
records=[]
def digest(path):
    with path.open('rb') as stream:return hashlib.file_digest(stream,'sha256').hexdigest()
binary_sha=digest(a.binary)
for scene in catalog['datasets']:
    for level in reversed(scene['levels']):
        key,divisor=level['key'],level['divisor']
        rep=scene['representations']['lossless'][key]
        width,height=rep['width'],rep['height']
        source=inputs[scene['id']][key]
        paths=[Path(a.input_prefix+path[len(a.strip_prefix):]) if a.strip_prefix and path.startswith(a.strip_prefix) else Path(path) for path in source['paths']]
        before=[{'path':str(path),'bytes':path.stat().st_size,'sha256':digest(path)} for path in paths]
        for quality,target in [('lossless',None),('lossy',80),('webmap',70)]:
            cell=a.output/scene['id']/key/quality
            cell.mkdir(parents=True,exist_ok=False)
            output=cell/'image.imijset'
            prefix='lossless' if target is None else 'lossy'
            geometry=[str(width),str(height)] if len(paths)==1 else [str(source['side'])]
            command=[str(a.binary),prefix+('-raw' if len(paths)==1 else '-mosaic'),*geometry,*map(str,paths),str(output)]
            if target is not None:command.append(str(target))
            record={'scene':scene['id'],'level':key,'divisor':divisor,'quality':quality,'target':target,'effort':10,'width':width,'height':height,'inputs':before,'binarySha256':binary_sha,'command':command}
            (cell/'command.json').write_text(json.dumps(record,indent=2)+'\n')
            started=time.monotonic()
            with (cell/'encode.log').open('xb') as log:result=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT)
            record.update(rc=result.returncode,seconds=time.monotonic()-started)
            (cell/'result.json').write_text(json.dumps(record,indent=2)+'\n')
            result.check_returncode()
            for path,item in zip(paths,before):
                if path.stat().st_size!=item['bytes'] or digest(path)!=item['sha256']:raise ValueError('input changed during encode')
            record.update(file=str(output.relative_to(a.output)),sha256=digest(output),encodedBytes=output.stat().st_size,indexSha256=digest(output.with_suffix('.bin')))
            records.append(record)
            (a.output/'preparation.json').write_text(json.dumps(records,indent=2)+'\n')
            print(scene['id'],key,quality,record['encodedBytes'],flush=True)
if digest(a.binary)!=binary_sha:raise ValueError('encoder changed')
(a.output/'complete.json').write_text(json.dumps({'count':len(records),'binarySha256':binary_sha})+'\n')
