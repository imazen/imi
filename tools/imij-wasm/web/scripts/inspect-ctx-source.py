"""Inspect one public CTX archive with bounded HTTP ranges, without decoding raster pixels."""
import argparse
import io
import json
from pathlib import Path
import ssl
import urllib.request
import zipfile
from PIL import Image

p=argparse.ArgumentParser()
p.add_argument('url')
p.add_argument('ca',type=Path)
p.add_argument('output',type=Path)
p.add_argument('--local',type=Path,help='Inspect a completely downloaded archive when TIFF metadata is at EOF')
a=p.parse_args()
a.output.mkdir(exist_ok=False)
context=ssl.create_default_context(cafile=str(a.ca))
requests=[]
class Remote(io.RawIOBase):
    def __init__(self):
        self.position=0
        with urllib.request.urlopen(urllib.request.Request(a.url,method='HEAD'),context=context,timeout=30) as r:
            self.length=int(r.headers['Content-Length'])
            self.etag=r.headers.get('ETag')
    def seekable(self):return True
    def readable(self):return True
    def tell(self):return self.position
    def seek(self,offset,whence=0):
        self.position=offset if whence==0 else self.position+offset if whence==1 else self.length+offset
        if self.position<0:raise ValueError('negative seek')
        return self.position
    def read(self,size=-1):
        if size<0:size=self.length-self.position
        size=min(size,self.length-self.position)
        if size==0:return b''
        if not 0<size<=8*1024*1024:raise ValueError('metadata request exceeds bounded inspection')
        start=self.position
        request=urllib.request.Request(a.url,headers={'Range':f'bytes={start}-{start+size-1}'})
        with urllib.request.urlopen(request,context=context,timeout=30) as r:
            if r.status!=206:raise ValueError('HTTP range required')
            if self.etag and r.headers.get('ETag')!=self.etag:raise ValueError('archive changed')
            data=r.read(size+1)
            if len(data)!=size:raise ValueError('HTTP extent')
        self.position+=size
        requests.append({'start':start,'bytes':size})
        return data
remote=None if a.local else Remote()
with zipfile.ZipFile(a.local or remote) as archive:
    inventory=[{'name':f.filename,'bytes':f.file_size,'compressedBytes':f.compress_size} for f in archive.infolist()]
    (a.output/'inventory.json').write_text(json.dumps(inventory,indent=2)+'\n')
    readme=next(f for f in archive.infolist() if f.filename.lower().endswith('readme.txt'))
    if readme.file_size>1024*1024:raise ValueError('README extent')
    (a.output/'README-source.txt').write_bytes(archive.read(readme))
    image=next(f for f in archive.infolist() if f.filename.endswith('_Mosaic.tif'))
    with archive.open(image) as stream:
        # Some GeoTIFFs keep IFD metadata at EOF. Only an explicitly supplied
        # complete local ZIP permits the decompression needed to reach it.
        source=stream if a.local else io.BytesIO(stream.read(1024*1024))
        Image.MAX_IMAGE_PIXELS=None
        with Image.open(source) as tif:
            dimensions={'width':tif.width,'height':tif.height,'mode':tif.mode,'pixels':tif.width*tif.height}
(a.output/'result.json').write_text(json.dumps({'url':a.url,'etag':remote.etag if remote else None,'archiveBytes':remote.length if remote else a.local.stat().st_size,'dimensions':dimensions,'requests':requests},indent=2)+'\n')
print(json.dumps(dimensions))
