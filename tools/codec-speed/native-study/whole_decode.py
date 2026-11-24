"""Single-band decode sensitivity only; no encoding or eight-worker claim."""
import pathlib,sys,json,subprocess,gzip,hashlib,statistics
root,program,zenbench=(pathlib.Path(s).resolve() for s in sys.argv[1:]);out=root/'whole-decode';out.mkdir(exist_ok=False)
rows=json.loads((root/'study-whole/preparation.json').read_text())['rows'];times={};aa=[]
for image in ('1000','1400','1600'):
 for q in (50,80):
  arms=[]
  for c in ('jpeg','native-rl','native-sh'):
   r=next(r for r in rows if r['image']==image and r['q']==q and r['codec']==c)
   arms.append({'id':c,'program':str(program),'args':['decode',c,str(q),str(r['effort']),'1',r['raw'],r['jpeg'],r['file'],r['reference']],'revision':'d75527f6 same candidate; one native band','timeout_seconds':60})
  p={'schema':1,'baseline':arms[0],'candidates':arms[1:],'cases':[{'id':image,'inputs':{}}],'metric':{'extract':{'kind':'prefix','prefix':'elapsed_ms='},'unit':'ms','expected':1,'divide_by':1},'check':{'kind':'json','prefix':'check=','pointers':['/source_sha256','/reference_ok','/pixels']},'warmup':'Three warmups and three allocating decodes per observation, single-threaded. Each output checked against its own retained raster.','blocks':3,'aa':True,'deadline_seconds':120}
  name=f'{image}-q{q}';plan=out/(name+'.json');plan.write_text(json.dumps(p,indent=2))
  with (out/(name+'.log')).open('x') as log:subprocess.run([str(zenbench),'pair',str(plan),'--output',str(out/name)],stdout=log,stderr=subprocess.STDOUT,check=True)
  packet=json.loads(gzip.decompress((out/name/'packet.json.gz').read_bytes()))
  if not packet['complete'] or len(packet['invocations'])!=12:raise RuntimeError('incomplete')
  if not all(s['complete_blocks']==3 and s['correctness_checked'] for s in packet['summaries']):raise RuntimeError('checks')
  for inv in packet['invocations']:
   if inv['exit_code']!=0 or not inv['continuation_safe']:raise RuntimeError('child failure')
   for f,h in inv['artifacts'].items():
    if hashlib.sha256(pathlib.Path(f).read_bytes()).hexdigest()!=h:raise RuntimeError('artifact changed')
  for n,c in enumerate(('jpeg','native-rl','native-sh')):times[(image,q,c)]=max(x['values_ns'][0] for x in packet['invocations'] if x['arm']==n)/1e6
  aa.extend({'case':name,**s} for s in packet['summaries'] if s['arm']=='__aa')
result=[]
for q in (50,80):
 for c in ('jpeg','native-rl','native-sh'):
  t=[times[(i,q,c)] for i in ('1000','1400','1600')];result.append({'q':q,'codec':c,'times_ms':t,'decode_1t_mpx_s':3*1048.576/sum(t)});print(result[-1],flush=True)
(out/'summary.json').write_text(json.dumps({'rows':result,'aa':aa,'invocations':72,'program_sha256':hashlib.sha256(program.read_bytes()).hexdigest()},indent=2))
