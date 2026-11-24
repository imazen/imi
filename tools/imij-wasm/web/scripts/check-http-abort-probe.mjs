// Isolate browser Fetch/body cancellation from the map and WASM decoder.
import {createServer} from 'node:http';
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,out,engine='firefox',mode='ordinary']=process.argv.slice(2);mkdirSync(out,{recursive:false});
const engines=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href),records=[];
const server=createServer((req,res)=>{
 if(req.url!=='/body'){res.writeHead(200,{'content-type':'text/html'});res.end('<!doctype html><title>HTTP abort probe</title>');return;}
 const record={closed:false};records.push(record);res.on('close',()=>{record.closed=true;record.finished=res.writableFinished;});
 res.writeHead(mode==='range'?206:200,{'content-length':'1000000','content-type':'application/octet-stream','cache-control':mode==='range'?'public,max-age=31536000,immutable':'no-store',...(mode==='range'?{'content-range':'bytes 0-999999/1000000','etag':'"fixed"'}:{})});res.write(new Uint8Array(100));
});
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
const browser=await engines[engine].launch();
try{
 const page=await browser.newPage();await page.goto(`http://127.0.0.1:${server.address().port}/`);
 const result=await page.evaluate(async(mode)=>{
  const controller=new AbortController(),response=await fetch('/body',{signal:controller.signal,cache:'no-store',...(mode==='range'?{headers:{Range:'bytes=0-999999','If-Range':'"fixed"'}}:{})}),reader=response.body.getReader();await reader.read();
  const pending=reader.read().then(()=>({done:true}),error=>({error:error.name}));
  controller.abort();const outcome=await pending;await reader.cancel().catch(()=>{});reader.releaseLock();return outcome;
 },mode);
 await new Promise(resolve=>setTimeout(resolve,1000));writeFileSync(resolve(out,'result.json'),JSON.stringify({engine,mode,result,records},null,2));console.log(JSON.stringify({engine,mode,result,records}));
}finally{await browser.close();server.closeAllConnections();await new Promise(resolve=>server.close(resolve));}
