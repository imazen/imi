"""Bounded native JPEG/effort study. All failures retain the current cell."""
import argparse, json, pathlib, subprocess, time, gzip, hashlib, sys
p=argparse.ArgumentParser();p.add_argument('probe');p.add_argument('raw');p.add_argument('zenbench');p.add_argument('output');p.add_argument('--prior-encode-budget-seconds',type=float,default=38.0);p.add_argument('--qualities',type=int,nargs='+',default=[5,15,30,50,65,80,95]);p.add_argument('--prepare-only',action='store_true');p.add_argument('--efforts',type=float,nargs='+',default=[0,3,5,7.5]);p.add_argument('--codecs',nargs='+',default=['direct','native-rl','native-sh']);p.add_argument('--band-rows',type=int,default=8);a=p.parse_args()
if not a.prepare_only and (a.band_rows!=8 or a.efforts!=[0,3,5,7.5] or a.codecs!=['direct','native-rl','native-sh']):raise ValueError('alternate configuration is preparation-only')
probe=pathlib.Path(a.probe).resolve();raw=pathlib.Path(a.raw).resolve();out=pathlib.Path(a.output).resolve();out.mkdir(exist_ok=False)
(out/'artifacts').mkdir();(out/'logs').mkdir();(out/'plans').mkdir();(out/'paired').mkdir()
(out/'source.json').write_text(json.dumps({'native_candidate':'d75527f658a55ae17e776a7622d970d9e4bded52','probe_sha256':hashlib.sha256(probe.read_bytes()).hexdigest(),'harness':'zenbench PR33 78a9dba0bdb4edfe09fd666ae0208944c3d2cd6d','prior_encode_budget_bound_seconds':a.prior_encode_budget_seconds},indent=2))
rows=[];used=a.prior_encode_budget_seconds
for image in ('1000','1400','1600'):
 for q in a.qualities:
  jpeg=out/'artifacts'/f'{image}-q{q}-jpeg-e0.bin'
  for codec,effort in [('jpeg',0)]+[(c,e) for c in a.codecs for e in a.efforts]:
   if used>=110:raise RuntimeError(f'Encode budget reached before preparation: {used}; retained rows {len(rows)}')
   name=f'{image}-q{q}-{codec}-e{effort:g}';file=out/'artifacts'/f'{name}.bin';ref=out/'artifacts'/f'{name}.rgb'
   cmd=[str(probe),'prepare',codec,str(q),str(effort),'1',str(raw/f'{image}.rgb'),str(jpeg),str(file),str(ref),str(a.band_rows)]
   (out/'logs'/f'{name}.command.json').write_text(json.dumps(cmd))
   with (out/'logs'/f'{name}.stdout').open('x') as stdout,(out/'logs'/f'{name}.stderr').open('x') as stderr:r=subprocess.run(cmd,stdout=stdout,stderr=stderr)
   (out/'logs'/f'{name}.rc').write_text(str(r.returncode));r.check_returncode()
   row=json.loads((out/'logs'/f'{name}.stdout').read_text());row.update(image=image,name=name,file=str(file),reference=str(ref),jpeg=str(jpeg),raw=str(raw/f'{image}.rgb'),band_rows=a.band_rows);used+=row['prepare_encode_ms']/1000
   for old in rows:
    if old['image']==image and old['q']==q and ((old['codec']==codec) or (old['codec'].startswith('native-') and codec.startswith('native-'))):
     if old['decoded_sha256']!=row['decoded_sha256']:raise RuntimeError(f'Unexpected effort/coder pixel difference {old["name"]} {name}')
   rows.append(row);(out/'preparation.json').write_text(json.dumps({'rows':rows,'encode_budget_used_seconds':used},indent=2))
   print(name,row['bytes'],row['ssim2'],f'encode budget {used:.3f}',flush=True)
if a.prepare_only:
 (out/'complete.json').write_text(json.dumps({'preparation_cells':len(rows),'plans':0,'encode_budget_used_seconds':used},indent=2));sys.exit(0)
# Comparisons use each arm's preserved pixels; source and exact-reference success
# are equal across arms, but differing codec reconstructions are not conflated.
for image in ('1000','1400','1600'):
 for q in (50,80):
  selected=[r for r in rows if r['image']==image and r['q']==q]
  for action in ('encode','decode'):
   if used>=110:raise RuntimeError(f'Encode budget reached before {image} q{q} {action}: {used}')
   arms=[]
   for r in selected:
    for threads in ((1,) if action=='encode' else (1,8)):
     arms.append({'id':f'{r["codec"]}-e{r["effort"]:g}-{threads}t','program':str(probe),'args':[action,r['codec'],str(q),str(r['effort']),str(threads),r['raw'],r['jpeg'],r['file'],r['reference']],'timeout_seconds':60,'revision':'d75527f6 native candidate; exact program SHA in source.json'})
   name=f'{image}-q{q}-{action}'
   plan={'schema':1,'baseline':arms[0],'candidates':arms[1:],'cases':[{'id':f'{image}-q{q}','inputs':{}}],'metric':{'extract':{'kind':'prefix','prefix':'elapsed_ms='},'unit':'ms','expected':1,'divide_by':1},'check':{'kind':'json','prefix':'check=','pointers':['/source_sha256','/reference_ok','/pixels']},'warmup':'Encode warmup once then one measured encode; decode three warmups then three allocating decodes per observation. Every arm checks its own retained exact pixels and encode bytes. Native encode includes JPEG coefficient parsing.','blocks':3,'aa':True,'deadline_seconds':600}
   path=out/'plans'/f'{name}.json';path.write_text(json.dumps(plan,indent=2))
   cmd=[a.zenbench,'pair',str(path),'--output',str(out/'paired'/name)]
   (out/'plans'/f'{name}.command.json').write_text(json.dumps(cmd))
   with (out/'logs'/f'{name}.stdout').open('x') as stdout,(out/'logs'/f'{name}.stderr').open('x') as stderr:r=subprocess.run(cmd,stdout=stdout,stderr=stderr)
   (out/'logs'/f'{name}.rc').write_text(str(r.returncode));r.check_returncode()
   packet=json.loads(gzip.decompress((out/'paired'/name/'packet.json.gz').read_bytes()))
   if not packet['complete']:raise RuntimeError('incomplete '+name)
   for inv in packet['invocations']:
    if inv['exit_code']!=0:raise RuntimeError('child failure '+name)
    if action=='encode':
     vals=[float(s.split('=',1)[1]) for s in (pathlib.Path(inv['output'])/'stdout.txt').read_text().splitlines() if s.startswith('encode_total_ms=')]
     if len(vals)!=1:raise RuntimeError('missing encode accounting')
     used+=vals[0]/1000
   (out/'budget.json').write_text(json.dumps({'prior_bound':a.prior_encode_budget_seconds,'used_seconds':used},indent=2));print('PASS',name,'budget',used,flush=True)
(out/'complete.json').write_text(json.dumps({'preparation_cells':len(rows),'plans':12,'encode_budget_used_seconds':used},indent=2))
