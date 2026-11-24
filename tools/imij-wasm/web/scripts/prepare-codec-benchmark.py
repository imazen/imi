"""Create 4K/8K browser fixtures from a shared original RGB8 crop; retain commands/hashes."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import time

p = argparse.ArgumentParser()
p.add_argument('source', type=Path)
p.add_argument('binary', type=Path)
p.add_argument('output', type=Path)
a = p.parse_args()
a.output.mkdir(exist_ok=False)

def sha(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()

records = []
def run(command, name):
    rec = {'command': list(map(str, command)), 'name': name}
    (a.output / f'{name}.command.json').write_text(json.dumps(rec, indent=2)+'\n')
    start = time.monotonic()
    with (a.output / f'{name}.log').open('xb') as log:
        result = subprocess.run(rec['command'], stdout=log, stderr=subprocess.STDOUT)
    rec.update(rc=result.returncode, elapsed_seconds=time.monotonic()-start)
    records.append(rec)
    (a.output / 'commands.json').write_text(json.dumps(records, indent=2)+'\n')
    result.check_returncode()

source_stat = a.source.stat()
if source_stat.st_size != 21600*21600*3:
    raise ValueError('original NASA RGB8 tile extent')
source_hash = sha(a.source)
source = {'file': str(a.source), 'sha256': source_hash, 'bytes': source_stat.st_size,
          'width': 21600, 'height': 21600, 'crop': {'x': 8000, 'y': 6000, 'width': 7680, 'height': 4320}}
(a.output / 'source.json').write_text(json.dumps(source, indent=2)+'\n')
raw8 = a.output / '8k.rgb'
with a.source.open('rb') as inp, raw8.open('xb') as out:
    for y in range(6000, 6000+4320):
        inp.seek((y*21600+8000)*3)
        row = inp.read(7680*3)
        if len(row) != 7680*3:
            raise ValueError('crop row extent')
        out.write(row)
raw4 = a.output / '4k.rgb'
run(['magick', '-size', '7680x4320', '-depth', '8', f'rgb:{raw8}', '-filter', 'box', '-resize', '3840x2160!', '-depth', '8', f'rgb:{raw4}'], 'resize-4k')
shutil.copy2(a.binary, a.output / 'browser_fixtures')
fixtures = []
for size, w, h in [('4k',3840,2160), ('8k',7680,4320)]:
    raw = a.output / f'{size}.rgb'
    if raw.stat().st_size != w*h*3:
        raise ValueError('prepared raster extent')
    cell = a.output / size
    run([a.binary, raw, w, h, cell], size+'-zen')
    run(['oxipng','-o','4','--out',cell/'crushed.png','--',cell/'compressed.png'],size+'-crush')
    common = ['ffmpeg','-nostdin','-v','error','-threads','8','-f','rawvideo','-pixel_format','rgb24','-video_size',f'{w}x{h}','-i',raw,'-frames:v','1']
    run([*common,'-c:v','libaom-av1','-still-picture','1','-cpu-used','8','-crf','30','-row-mt','1','-tiles','2x2','-pix_fmt','yuv420p','-threads','8',cell/'image.avif'],size+'-avif')
    for name, distance in [('lossless',0),('lossy',1)]:
        run([*common,'-c:v','libjxl','-effort','3','-distance',distance,'-threads','8',cell/f'{name}.jxl'],size+'-jxl-'+name)
    run(['cwebp','-quiet','-lossless','-m','1','-mt',cell/'compressed.png','-o',cell/'lossless.webp'],size+'-webp-lossless')
    run(['cwebp','-quiet','-q','80','-m','4','-mt',cell/'compressed.png','-o',cell/'lossy.webp'],size+'-webp-lossy')
    run(['magick','-limit','memory','2GiB','-limit','map','0','-define',f'registry:temporary-path={a.output}','-size',f'{w}x{h}','-depth','8',f'rgb:{raw}','-colors','256',cell/'image.gif'],size+'-gif')
    formats = [('jpeg','Progressive JPEG','progressive.jpg','image/jpeg','zenjpeg q80 · 4:2:0 · progressive'),
               ('png','Crushed PNG','crushed.png','image/png','zenpng effort 9 + oxipng -o4 · lossless RGB8'),
               ('avif','AVIF','image.avif','image/avif','libaom cpu-used 8 · CRF 30 · 4:2:0'),
               ('jxl-lossless','JPEG XL lossless','lossless.jxl','image/jxl','libjxl effort 3 · distance 0'),
               ('jxl-lossy','JPEG XL lossy','lossy.jxl','image/jxl','libjxl effort 3 · distance 1'),
               ('webp-lossless','WebP lossless','lossless.webp','image/webp','libwebp lossless · method 1'),
               ('webp-lossy','WebP lossy','lossy.webp','image/webp','libwebp quality 80 · method 4'),
               ('gif','GIF','image.gif','image/gif','ImageMagick · 256-color quantization')]
    for codec,label,file,mime,setting in formats:
        path = cell/file
        # Full native reconstruction, including dimensions, before publishing.
        decoded = a.output/f'{size}-{codec}.decoded.rgb'
        run(['ffmpeg','-nostdin','-v','error','-threads','8','-i',path,'-frames:v','1','-threads','8','-pix_fmt','rgb24','-f','rawvideo',decoded],size+'-verify-'+codec)
        if decoded.stat().st_size != w*h*3:
            raise ValueError('fixture decoded raster extent')
        reference = sha(decoded)
        if codec in ('png','jxl-lossless','webp-lossless') and reference != sha(raw):
            raise ValueError('lossless fixture pixel mismatch: '+codec)
        fixtures.append({'id':f'{size}-{codec}','codec':codec,'label':label,'sizeLabel':size.upper(),'width':w,'height':h,
                         'file':str(path.relative_to(a.output)),'bytes':path.stat().st_size,'sha256':sha(path),'mime':mime,
                         'setting':setting,'decodedRgbSha256':reference,'sourceRgbSha256':sha(raw)})
if a.source.stat().st_mtime_ns != source_stat.st_mtime_ns or sha(a.source) != source_hash:
    raise ValueError('source changed')
(a.output/'fixtures.json').write_text(json.dumps({'source':source,'fixtures':fixtures},indent=2)+'\n')
print(f'{len(fixtures)} native-validated fixtures; all six lossless rasters exact',flush=True)
