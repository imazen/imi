"""Bounded full-raster SSIMULACRA2 matching; preserve every candidate and command."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import shutil
import subprocess
import time

p=argparse.ArgumentParser()
p.add_argument('prior',type=Path)
p.add_argument('binary',type=Path)
p.add_argument('output',type=Path)
p.add_argument('--fixtures-binary',type=Path,required=True)
a=p.parse_args()
a.binary=a.binary.resolve()
a.output.mkdir(exist_ok=False)
prior=json.loads((a.prior/'fixtures.json').read_text())
fixtures=[]
records=[]
TOLERANCE=.5

def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream,'sha256').hexdigest()

def run(command,stem):
    command=list(map(str,command))
    (a.output/f'{stem}.command.json').write_text(json.dumps(command)+'\n')
    start=time.monotonic()
    with (a.output/f'{stem}.log').open('xb') as log:
        result=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT)
    record={'name':stem,'command':command,'rc':result.returncode,'seconds':time.monotonic()-start}
    records.append(record)
    (a.output/'commands.json').write_text(json.dumps(records,indent=2)+'\n')
    result.check_returncode()

shutil.copy2(a.binary,a.output/'browser_match')
for size,w,h in [('4k',3840,2160),('8k',7680,4320)]:
    cell=a.output/size
    cell.mkdir()
    raw=a.prior/f'{size}.rgb'
    raw_sha=sha(raw)
    stderr=(cell/'metric.stderr').open('xb')
    metric_command=[str(a.binary),'score',str(raw),str(w),str(h)]
    (cell/'metric-command.json').write_text(json.dumps(metric_command)+'\n')
    metric=subprocess.Popen(metric_command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=stderr,text=True,bufsize=1)
    scores=[]
    def score(path):
        if path.stat().st_size!=w*h*3:
            raise ValueError('score raster extent')
        metric.stdin.write(str(path)+'\n')
        metric.stdin.flush()
        line=metric.stdout.readline()
        if not line:
            raise RuntimeError('metric process failed; see metric.stderr')
        value=float(line)
        if not math.isfinite(value):
            raise ValueError('nonfinite quality')
        scores.append({'file':str(path),'sha256':sha(path),'ssim2':value})
        (cell/'scores.json').write_text(json.dumps(scores,indent=2)+'\n')
        return value
    try:
        self_score=score(raw)
        if abs(self_score-100)>.001:
            raise ValueError('metric identity control')
        run([a.fixtures_binary.resolve(),raw,w,h,cell/'standard-max'],size+'-jpeg-png-standard-max')
        jpeg=cell/'standard-max/progressive.jpg'
        jpeg_ref=cell/'jpeg-max.rgb'
        run(['ffmpeg','-nostdin','-v','error','-threads','8','-i',jpeg,'-frames:v','1','-threads','8','-pix_fmt','rgb24','-f','rawvideo',jpeg_ref],size+'-jpeg-max-decode')
        anchor=score(jpeg_ref)
        print(size,'anchor',anchor,flush=True)
        for old in prior['fixtures']:
            if old['sizeLabel']!=size.upper() or old['codec'] not in ('jpeg','png','jxl-lossless','webp-lossless'):
                continue
            file=cell/'standard-max'/('progressive.jpg' if old['codec']=='jpeg' else 'compressed.png') if old['codec'] in ('jpeg','png') else cell/Path(old['file']).name
            if old['codec']=='jxl-lossless':
                run(['ffmpeg','-nostdin','-v','error','-threads','8','-f','rawvideo','-pixel_format','rgb24','-video_size',f'{w}x{h}','-i',raw,'-frames:v','1','-c:v','libjxl','-effort','9','-distance','0','-threads','8',file],size+'-jxl-lossless-max')
            elif old['codec']=='webp-lossless':
                run(['cwebp','-quiet','-lossless','-z','9','-m','6','-mt',cell/'standard-max/compressed.png','-o',file],size+'-webp-lossless-max')
            target=cell/file.name
            if file!=target:shutil.copy2(file,target)
            fixture=dict(old,file=str(target.relative_to(a.output)),bytes=target.stat().st_size,sha256=sha(target),lossless=old['codec']!='jpeg',setting={'jpeg':'zenjpeg Max · q80 · progressive 4:2:0','png':'zenpng effort 30','jxl-lossless':'libjxl effort 9 · distance 0','webp-lossless':'libwebp lossless preset 9 · method 6'}[old['codec']])
            if old['codec']=='jpeg':fixture['decodedRgbSha256']=sha(jpeg_ref)
            else:
                check=cell/(old['codec']+'-max.rgb')
                run(['ffmpeg','-nostdin','-v','error','-threads','8','-i',target,'-frames:v','1','-threads','8','-pix_fmt','rgb24','-f','rawvideo',check],size+'-'+old['codec']+'-max-decode')
                if sha(check)!=raw_sha:raise ValueError('lossless comparator pixels')
                fixture['decodedRgbSha256']=sha(check)
            if old['codec']=='jpeg':
                fixture.update(ssim2=anchor,targetSsim2=anchor,qualityTolerance=TOLERANCE)
            fixtures.append(fixture)
        for mode in ['lossless','lossy']:
            if mode=='lossless':
                file=cell/'lossless.imi'
                ref=cell/'lossless-imi.rgb'
                run([a.binary,'encode','lossless',0,raw,w,h,file,ref],size+'-imi-lossless')
                if sha(ref)!=raw_sha:
                    raise ValueError('lossless IMI source pixels')
                fixtures.append({'id':size+'-imi-lossless','codec':'imi-lossless','label':'IMI · WASM SIMD · 1 worker','sizeLabel':size.upper(),'width':w,'height':h,'file':str(file.relative_to(a.output)),'bytes':file.stat().st_size,'sha256':sha(file),'mime':'application/octet-stream','setting':'IMI default lossless · effort 10','decodedRgbSha256':sha(ref),'sourceRgbSha256':raw_sha,'lossless':True})
        for codec,ext,mime,label,lo,hi,increasing,initial in [
            ('avif','avif','image/avif','AVIF',0,63,False,30),
            ('jxl-lossy','jxl','image/jxl','JPEG XL',.01,12,False,1),
            ('webp-lossy','webp','image/webp','WebP',0,100,True,80),
            ('imi-lossy','imi','application/octet-stream','IMI · WASM SIMD · 1 worker',0,100,True,80)]:
            trials=[]
            best=None
            tried=set()
            q=initial
            for attempt in range(14):
                if codec=='avif':
                    q=round(q)
                key=round(q,8)
                if key in tried:
                    break
                tried.add(key)
                stem=f'{codec}-{attempt:02}'
                file=cell/(stem+'.'+ext)
                ref=cell/(stem+'.rgb')
                if codec=='imi-lossy':
                    run([a.binary,'encode','lossy',q,raw,w,h,file,ref],size+'-'+stem)
                else:
                    common=['ffmpeg','-nostdin','-v','error','-threads','8','-f','rawvideo','-pixel_format','rgb24','-video_size',f'{w}x{h}','-i',raw,'-frames:v','1']
                    if codec=='avif':
                        command=[*common,'-c:v','libaom-av1','-still-picture','1','-cpu-used','0','-crf',q,'-row-mt','1','-tiles','2x2','-pix_fmt','yuv420p','-threads','8',file]
                    elif codec=='jxl-lossy':
                        command=[*common,'-c:v','libjxl','-effort','9','-distance',q,'-threads','8',file]
                    else:
                        command=['cwebp','-quiet','-q',q,'-m','6','-mt',cell/'standard-max/compressed.png','-o',file]
                    run(command,size+'-'+stem)
                    run(['ffmpeg','-nostdin','-v','error','-threads','8','-i',file,'-frames:v','1','-threads','8','-pix_fmt','rgb24','-f','rawvideo',ref],size+'-'+stem+'-decode')
                value=score(ref)
                trial={'quality':q,'ssim2':value,'file':str(file),'reference':str(ref),'bytes':file.stat().st_size}
                trials.append(trial)
                if best is None or abs(value-anchor)<abs(best['ssim2']-anchor):
                    best=trial
                (cell/(codec+'-search.json')).write_text(json.dumps({'target':anchor,'tolerance':TOLERANCE,'trials':trials,'best':best},indent=2)+'\n')
                print(size,codec,q,value,'delta',value-anchor,flush=True)
                if abs(value-anchor)<=TOLERANCE:
                    break
                if (value<anchor)==increasing:
                    lo=q if codec!='avif' else q+1
                else:
                    hi=q if codec!='avif' else q-1
                if lo>hi:
                    break
                q=(lo+hi)/2
            if best is None or abs(best['ssim2']-anchor)>TOLERANCE:
                raise ValueError(f'{size} {codec} cannot meet fixed SSIMULACRA2 tolerance')
            file=Path(best['file'])
            q=best['quality']
            settings={'avif':f'libaom cpu-used 0 · CRF {q:g} · 4:2:0','jxl-lossy':f'libjxl effort 9 · distance {q:g}','webp-lossy':f'libwebp q{q:g} · method 6','imi-lossy':f'IMI Fast · effort 10 · JPEG-quality {q:g}'}
            fixtures.append({'id':size+'-'+codec,'codec':codec,'label':label,'sizeLabel':size.upper(),'width':w,'height':h,'file':str(file.relative_to(a.output)),'bytes':file.stat().st_size,'sha256':sha(file),'mime':mime,'setting':settings[codec],'decodedRgbSha256':sha(Path(best['reference'])),'sourceRgbSha256':raw_sha,'lossless':False,'ssim2':best['ssim2'],'targetSsim2':anchor,'qualityTolerance':TOLERANCE})
    finally:
        metric.stdin.close()
        rc=metric.wait()
        stderr.close()
        (cell/'metric.rc').write_text(str(rc)+'\n')
        if rc:
            raise RuntimeError('metric helper failed')
fixtures.sort(key=lambda f:(f['width'],f['id']))
if len(fixtures)!=18:
    raise ValueError('fixture inventory')
(a.output/'fixtures.json').write_text(json.dumps({'source':prior['source'],'qualityMetric':'SSIMULACRA2','qualityTolerance':TOLERANCE,'fixtures':fixtures},indent=2)+'\n')
print('18 fixtures; all ten lossy fixtures matched within fixed 0.5 SSIMULACRA2 points',flush=True)
