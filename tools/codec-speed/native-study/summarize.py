"""Audit native study; PCHIP log-rate integration with linear sensitivity."""
import gzip, hashlib, json, math, pathlib, statistics, sys
from scipy.interpolate import PchipInterpolator
import numpy as np

def bd(base,candidate,band=None):
    def curve(rows):
        ordered=sorted(rows,key=lambda r:r['ssim2'])
        x=np.array([r['ssim2'] for r in ordered]);y=np.log([r['bytes'] for r in ordered])
        if len(x)<4 or not np.isfinite(x).all() or not np.isfinite(y).all() or not (np.diff(x)>0).all():raise ValueError('invalid curve')
        if not (np.diff(y)>0).all():raise ValueError('nonmonotonic measured rate; preserve and inspect rather than prune')
        return x,y
    x,y=curve(base);u,v=curve(candidate);lo=max(x[0],u[0]);hi=min(x[-1],u[-1])
    if band is not None:
        if lo>band[0] or hi<band[1]:return {'supported':False,'available_overlap':[float(lo),float(hi)]}
        lo,hi=band
    if hi<=lo:return {'supported':False,'available_overlap':[float(lo),float(hi)]}
    p=(PchipInterpolator(u,v,extrapolate=False).integrate(lo,hi)-PchipInterpolator(x,y,extrapolate=False).integrate(lo,hi))/(hi-lo)
    def lin(a,b):
        knots=np.array(sorted({float(lo),float(hi),*(float(t) for t in a if lo<t<hi)}));return np.trapezoid(np.interp(knots,a,b),knots)
    l=(lin(u,v)-lin(x,y))/(hi-lo)
    return {'supported':True,'range':[float(lo),float(hi)],'log_ratio':float(p),'bd_rate_pct':100*math.expm1(p),'linear_bd_rate_pct':100*math.expm1(l)}
# Numerical controls: equal curves and a known constant multiplicative rate.
control=[{'ssim2':q,'bytes':math.exp(q/50)*100} for q in (0,25,50,75,100)]
for factor in (1.,1.05,.9):
    r=bd(control,[{**p,'bytes':p['bytes']*factor} for p in control],(10,80))
    if abs(r['bd_rate_pct']-(factor-1)*100)>1e-10:raise RuntimeError('BD numerical control')
root=pathlib.Path(sys.argv[1]).resolve();prep=json.loads((root/'preparation.json').read_text());rows=prep['rows']
if len(rows)!=273 or not (root/'complete.json').exists():raise RuntimeError('incomplete study')
if len(sys.argv)>2:
 extra=pathlib.Path(sys.argv[2]);more=json.loads((extra/'preparation.json').read_text())['rows']
 if len(more)!=39 or not (extra/'complete.json').exists():raise RuntimeError('incomplete q1 extension')
 rows+=more
 if len({r['name'] for r in rows})!=len(rows):raise RuntimeError('duplicate cells')
extra_jpeg_seconds=0.0
if len(sys.argv)>3:
 whole=pathlib.Path(sys.argv[3]);items=json.loads((whole/'preparation.json').read_text())['rows']
 if len(items)!=72 or not (whole/'complete.json').exists():raise RuntimeError('incomplete whole-band preparation')
 for r in items:
  old=next(x for x in rows if x['image']==r['image'] and x['q']==r['q'] and x['codec']==r['codec'] and (r['codec']=='jpeg' or x['effort']==r['effort']))
  if r['source_sha256']!=old['source_sha256'] or r['decoded_sha256']!=old['decoded_sha256']:raise RuntimeError('band size changed pixels/source')
  if r['codec']=='jpeg':
   if r['encoded_sha256']!=old['encoded_sha256']:raise RuntimeError('JPEG preparation changed')
   extra_jpeg_seconds+=r['prepare_encode_ms']/1000
  else:
   rows.append({**r,'codec':r['codec']+'-whole'})
for r in rows:
    for p,k in ((r['file'],'encoded_sha256'),(r['reference'],'decoded_sha256'),(r['raw'],'source_sha256')):
        if hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()!=r[k]:raise RuntimeError('changed artifact '+p)
    if not math.isfinite(r['ssim2']):raise RuntimeError('metric')
quality=[]
for codec in ('direct','native-rl','native-sh')+(('native-rl-whole','native-sh-whole') if len(sys.argv)>3 else ()):
 for effort in ((7.5,) if codec.endswith('-whole') else (0,3,5,7.5)):
  for band in (None,(10,50)):
   per=[]
   for image in ('1000','1400','1600'):
    b=[r for r in rows if r['image']==image and r['codec']=='jpeg'];c=[r for r in rows if r['image']==image and r['codec']==codec and r['effort']==effort]
    per.append({'image':image,**bd(b,c,band)})
   complete=all(r['supported'] for r in per)
   quality.append({'codec':codec,'effort':effort,'band':'common' if band is None else '10-50','images':per,'complete':complete,'geomean_bd_rate_pct':100*math.expm1(statistics.mean(r['log_ratio'] for r in per)) if complete else None})
measurements={};aa=[];invocations=0;totalencode=0
for path in sorted((root/'paired').glob('*/packet.json.gz')):
 p=json.loads(gzip.decompress(path.read_bytes()));name=path.parent.name;image,q,action=name.split('-');q=int(q[1:]);arms=[p['plan']['baseline'],*p['plan']['candidates'],{'id':'__aa'}]
 if not p['complete'] or p['plan']['blocks']!=3:raise RuntimeError('incomplete packet')
 if not all(s['correctness_checked'] and s['complete_blocks']==3 for s in p['summaries']):raise RuntimeError('failed checks')
 cells={}
 for r in p['invocations']:
  if r['exit_code']!=0 or not r['continuation_safe'] or len(r['values_ns'])!=1 or r['values_ns'][0]<=0:raise RuntimeError('bad child')
  for file,sha in r['artifacts'].items():
   if hashlib.sha256(pathlib.Path(file).read_bytes()).hexdigest()!=sha:raise RuntimeError('changed child evidence')
  arm=arms[r['arm']]['id'];cells.setdefault(arm,[]).append(r['values_ns'][0]/1e6);invocations+=1
  if action=='encode':
   vals=[float(s.split('=',1)[1]) for s in (pathlib.Path(r['output'])/'stdout.txt').read_text().splitlines() if s.startswith('encode_total_ms=')]
   if len(vals)!=1:raise RuntimeError('missing budget')
   totalencode+=vals[0]/1000
 for arm,times in cells.items():
  if len(times)!=3:raise RuntimeError('missing block')
  measurements[(image,q,action,arm)]=max(times)
 aa.extend({'plan':name,**s} for s in p['summaries'] if s['arm']=='__aa')
if invocations!=738 or len(aa)!=12:raise RuntimeError((invocations,len(aa)))
speed=[]
for q in (50,80):
 for codec,effort in [('jpeg',0)]+[(c,e) for c in ('direct','native-rl','native-sh') for e in (0,3,5,7.5)]:
  arm=f'{codec}-e{effort:g}';selected=[r for r in rows if r['q']==q and r['codec']==codec and r['effort']==effort]
  item={'codec':codec,'q':q,'effort':effort,'bpp':statistics.mean(r['bpp'] for r in selected),'ssim2_by_image':{r['image']:r['ssim2'] for r in selected}}
  times=[measurements[(i,q,'encode',arm+'-1t')] for i in ('1000','1400','1600')];item['encode_ms_mean']=statistics.mean(times);item['encode_mpx_s']=1048.576/item['encode_ms_mean']
  if codec.startswith('native-'):
   jpeg=statistics.mean(measurements[(i,q,'encode','jpeg-e0-1t')] for i in ('1000','1400','1600'));item['rgb_pipeline_ms_mean']=jpeg+item['encode_ms_mean'];item['rgb_pipeline_mpx_s']=1048.576/item['rgb_pipeline_ms_mean']
  for t in (1,8):
   per=[measurements[(i,q,'decode',arm+f'-{t}t')] for i in ('1000','1400','1600')];item[f'decode_{t}t_ms_by_image']=per;item[f'decode_{t}t_mpx_s']=3*1048.576/sum(per)
  speed.append(item)
res={'quality':quality,'speed':speed,'invocations':invocations,'aa':aa,'new_encode_seconds':sum(r['prepare_encode_ms'] for r in rows)/1000+totalencode+extra_jpeg_seconds,'budget':json.loads((root/'budget.json').read_text()),'max_native_rgb_difference':max(r['max_rgb_diff_from_jpeg'] for r in rows if r['codec'].startswith('native-'))}
with (root/('summary-with-bands.json' if len(sys.argv)>3 else 'summary.json')).open('x') as f:json.dump(res,f,indent=2)
for r in quality:
 if r['band']=='10-50':print('BD10-50',r['codec'],r['effort'],r['geomean_bd_rate_pct'])
for r in speed:print(r['q'],r['codec'],r['effort'],'bpp',round(r['bpp'],3),'enc',round(r['encode_mpx_s'],1),'dec',round(r['decode_1t_mpx_s']),round(r['decode_8t_mpx_s']))
print('New encode seconds',res['new_encode_seconds'],'native max RGB difference',res['max_native_rgb_difference'])
