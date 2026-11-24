"""Publish native-validated benchmark inputs as uncached, content-addressed objects."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
from urllib.parse import urlsplit
p=argparse.ArgumentParser()
p.add_argument('fixtures',type=Path)
p.add_argument('output',type=Path)
p.add_argument('--bucket',default='codec-corpus')
p.add_argument('--prefix',default='imij/browser-codec-benchmark')
p.add_argument('--public-base',default='https://codec-corpus.r2.imazen.org')
p.add_argument('--source-bucket',help='Copy an existing published catalog from this bucket instead of uploading local fixtures')
a=p.parse_args()
if a.output.exists():
    raise FileExistsError(a.output)
if not a.prefix or any(part in ('', '.', '..') for part in a.prefix.split('/')):
    raise ValueError('object prefix')
data=json.loads((a.fixtures if a.source_bucket else a.fixtures/'fixtures.json').read_text())
endpoint='https://'+os.environ['R2_ACCOUNT_ID']+'.r2.cloudflarestorage.com'
os.environ['AWS_ACCESS_KEY_ID']=os.environ['R2_ACCESS_KEY_ID']
os.environ['AWS_SECRET_ACCESS_KEY']=os.environ['R2_SECRET_ACCESS_KEY']
os.environ.pop('AWS_SESSION_TOKEN',None)
for f in data['fixtures']:
    digest=f['sha256']
    if a.source_bucket:
        source=urlsplit(f['url']).path.lstrip('/')
        name=Path(source).name
        if source!='imij/browser-codec-benchmark/'+digest+'/'+name:
            raise ValueError('unexpected source fixture key')
        head=json.loads(subprocess.check_output(['aws','--endpoint-url',endpoint,'s3api','head-object','--bucket',a.source_bucket,'--key',source]))
        if head['ContentLength']!=f['bytes'] or head['Metadata']['sha256']!=digest:
            raise ValueError('source fixture identity')
    else:
        file=a.fixtures/f['file']
        name=file.name
        with file.open('rb') as stream:
            actual=hashlib.file_digest(stream,'sha256').hexdigest()
        if actual!=digest or file.stat().st_size!=f['bytes']:
            raise ValueError('fixture changed')
    key=a.prefix+'/'+digest+'/'+name
    if a.source_bucket:
        subprocess.run(['aws','--endpoint-url',endpoint,'s3api','copy-object','--bucket',a.bucket,'--key',key,'--copy-source',a.source_bucket+'/'+source,'--metadata-directive','REPLACE','--content-type',f['mime'],'--cache-control','no-store','--metadata','sha256='+digest],check=True,stdout=subprocess.DEVNULL)
    else:
        subprocess.run(['aws','--endpoint-url',endpoint,'s3','cp',str(file),'s3://'+a.bucket+'/'+key,'--no-progress','--only-show-errors','--content-type',f['mime'],'--cache-control','no-store','--metadata','sha256='+digest],check=True)
    head=json.loads(subprocess.check_output(['aws','--endpoint-url',endpoint,'s3api','head-object','--bucket',a.bucket,'--key',key]))
    if head['ContentLength']!=f['bytes'] or head['Metadata']['sha256']!=digest or head['CacheControl']!='no-store':
        raise ValueError('published identity or caching policy')
    f['url']=a.public_base.rstrip('/')+'/'+key
    f.pop('file',None)
    print(f['id'],f['bytes'],digest,flush=True)
if not a.source_bucket:
    data['source']={'credit':'NASA Earth Observatory / Reto Stöckli','url':'https://visibleearth.nasa.gov/images/73751/july-blue-marble-next-generation-w-topography-and-bathymetry','rgbSha256':data['source']['sha256'],'crop':data['source']['crop'],'resize4k':'Box filter, 2:1, encoded RGB8'}
a.output.write_text(json.dumps(data,indent=2)+'\n')
