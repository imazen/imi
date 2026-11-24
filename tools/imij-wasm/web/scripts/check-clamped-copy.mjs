// Diagnose binding-copy cost; these are not image decoder throughput numbers.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,output,engines='chromium,firefox,webkit']=process.argv.slice(2);mkdirSync(output,{recursive:false});
const browsers=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href),results=[];
for(const engine of engines.split(',')){
 const browser=await browsers[engine].launch();
 try{
  const page=await browser.newPage();await page.goto(url);
  const rows=await page.evaluate(()=>{
   const length=7680*128*4,memory=new WebAssembly.Memory({initial:64}),source=new Uint8Array(memory.buffer,0,length);source.fill(127);source[length-1]=233;
   const rows=[];
   for(let trial=0;trial<20;trial++)for(const path of trial%2?['set','constructor']:['constructor','set']){
    const start=performance.now();let result;
    if(path==='constructor')result=new Uint8ClampedArray(source);else{result=new Uint8ClampedArray(length);result.set(source);}
    const ms=performance.now()-start;
    for(let i=0;i<length;i++)if(result[i]!==source[i])throw Error('copy mismatch');
    rows.push({trial,path,length,ms});
   }
   return rows;
  });
  results.push({engine,version:browser.version(),rows});writeFileSync(resolve(output,'results.json'),JSON.stringify(results,null,2),{flag:'w'});
 }finally{await browser.close();}
}
