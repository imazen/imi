// Freeze complete pyramid geometry, original camera trajectory and exact sources.
import {readFileSync,writeFileSync,mkdirSync,copyFileSync} from 'node:fs';
import {resolve,join} from 'node:path';
import {createHash} from 'node:crypto';
import {pyramidPlan} from './access-pyramid-model.mjs';
const [captureFile,existing,out,budgetText='16777216',geometry='both']=process.argv.slice(2);
if(!['both','captured'].includes(geometry))throw Error('geometry must be both or captured');
if(!out)throw Error('CAPTURE EXISTING_INPUT_DIRECTORY OUTPUT [MAX_BYTES]');
mkdirSync(out,{recursive:false});mkdirSync(join(out,'objects'));
const capture=JSON.parse(readFileSync(captureFile)),budget=Number(budgetText),digest=b=>createHash('sha256').update(b).digest('hex');
if(!Number.isSafeInteger(budget)||budget<=0)throw Error('source budget');
const plans=[];
for(const [name,dppx,auto] of (geometry==='captured'?[['captured',null,false]]:[['captured',null,false],['auto-dpr2',2,true]]))for(const cache of ['none','retained']){
 plans.push({name:`${name}-${cache}`,...pyramidPlan(capture,{dppx,auto,decodedBudget:cache==='none'?0:64*1024**2,displayBudget:cache==='none'?0:32*1024**2})});
}
const reps=new Map(plans.flatMap(p=>p.views.map(v=>[v.rep.sha256,v.rep]))),total=[...reps.values()].reduce((n,r)=>n+r.encodedBytes,0);
if(total>budget)throw Error(`source bytes ${total} exceed budget ${budget}`);
const inventory=[];
for(const [sha,rep] of reps){
 if(rep.parts)throw Error('CPU calibrator currently requires complete resident logical files');
 const sourceUrl=new URL(rep.url,capture.url).href,indexUrl=new URL(rep.index.url,capture.url).href;
 let bytes;try{bytes=readFileSync(join(existing,`${sha}.imiset`));}catch(e){if(e.code!=='ENOENT')throw e;const response=await fetch(sourceUrl);if(!response.ok)throw Error(`source HTTP ${response.status}`);bytes=Buffer.from(await response.arrayBuffer());}
 if(bytes.length!==rep.encodedBytes||digest(bytes)!==sha)throw Error('source identity');
 writeFileSync(join(out,'objects',`${sha}.imiset`),bytes,{flag:'wx'});
 const response=await fetch(indexUrl);if(!response.ok)throw Error(`index HTTP ${response.status}`);const index=Buffer.from(await response.arrayBuffer());
 if(digest(index)!==rep.index.sha256||index.length!==rep.index.bytes)throw Error('index identity');
 writeFileSync(join(out,'objects',`${sha}.index`),index,{flag:'wx'});
 inventory.push({sha256:sha,bytes:bytes.length,indexSha256:digest(index),indexBytes:index.length,sourceUrl,indexUrl});
}
for(const p of plans){
 const lines=p.views.map(v=>[v.label,v.arrival_ms,resolve(out,'objects',v.rep.sha256+'.imiset'),'archive',v.keys.join(',')||'-',0,0,0,0].join('\t'));
 writeFileSync(join(out,p.name+'.tsv'),lines.join('\n')+'\n',{flag:'wx'});
}
writeFileSync(join(out,'plans.json'),JSON.stringify({schema:1,captureSha256:digest(readFileSync(captureFile)),captureUrl:capture.url,inventory,plans},null,2),{flag:'wx'});
copyFileSync(captureFile,join(out,'capture.json'));
console.log(`${plans.length} pyramid/cache policies, ${reps.size} verified logical objects, ${total} encoded bytes`);
