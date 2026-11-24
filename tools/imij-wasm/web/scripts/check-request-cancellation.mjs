// A real HTTP response body stalls until the browser cancels obsolete views.
import assert from 'node:assert/strict';
import {createServer} from 'node:http';
import {Readable} from 'node:stream';
import {pipeline} from 'node:stream/promises';
import {mkdirSync,writeFileSync,readFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out,engine='chromium']=process.argv.slice(2);mkdirSync(out,{recursive:false});
const engines=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const requests=[],events=[];let initial,afterZoom,port,crossPort;
const handler=async(req,res)=>{
 const path=new URL(req.url,'http://fixture').pathname;
 if(path!=='/archive'){
  try{
   if(path==='/image/image.json'){
    const catalog=JSON.parse(readFileSync(resolve(web,'image/image.json'),'utf8'));
    const proxy=(source,stall=false)=>`http://127.0.0.1:${stall?crossPort:port}/archive?source=${encodeURIComponent(new URL(source,url))}${stall?'&stall=1':''}`;
    for(const scene of catalog.datasets)for(const [quality,levels] of Object.entries(scene.representations))for(const rep of Object.values(levels)){
     rep.url=proxy(rep.url||rep.file,quality==='lossless');if(rep.index)rep.index.url=proxy(rep.index.url);if(rep.bootstrap)rep.bootstrap.url=proxy(rep.bootstrap.url);
    }
    res.writeHead(200,{'content-type':'application/json','cache-control':'no-store'});res.end(JSON.stringify(catalog));return;
   }
   const file=resolve(web,'.'+(path.endsWith('/')?path+'index.html':path));if(!file.startsWith(resolve(web)+'/'))throw Error('fixture path');
   const type=file.endsWith('.js')?'text/javascript':file.endsWith('.wasm')?'application/wasm':file.endsWith('.css')?'text/css':file.endsWith('.html')?'text/html':'application/json';
   const body=readFileSync(file);res.writeHead(200,{'content-type':type,'cache-control':'no-store'});res.end(body);
  }catch{res.writeHead(404);res.end();}return;
 }
 if(req.method==='OPTIONS'){res.writeHead(204,{'access-control-allow-origin':'*','access-control-allow-methods':'GET,HEAD,OPTIONS','access-control-allow-headers':'Range,If-Range'});res.end();return;}
 const stalled=new URL(req.url,'http://fixture').searchParams.get('stall')==='1';
 const record={url:req.url,method:req.method,range:req.headers.range,closed:false,finished:false};if(stalled)requests.push(record);
 const controller=new AbortController();
 res.on('close',()=>{record.closed=true;record.finished=res.writableFinished;controller.abort();});
 try{
  const target=new URL(req.url,'http://fixture').searchParams.get('source');
  const headers=req.headers.range?{Range:req.headers.range,'If-Range':req.headers['if-range']}:{};
  const upstream=await fetch(target,{method:req.method,headers,signal:controller.signal});
  res.writeHead(upstream.status,{...Object.fromEntries(upstream.headers),'access-control-allow-origin':'*','access-control-expose-headers':'*'});res.flushHeaders();
  if(req.method==='HEAD'){res.end();return;}
  if(!stalled){await pipeline(Readable.fromWeb(upstream.body),res);return;}
  // Keep a genuine body open, with one received byte, until superseded.
  const reader=upstream.body.getReader(),first=await reader.read();
  if(!first.done)res.write(first.value.subarray(0,1));
  await reader.cancel();
 }catch(error){if(!controller.signal.aborted)res.destroy(error);}
};
const server=createServer(handler),crossServer=createServer(handler);
await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
await new Promise(resolve=>crossServer.listen(0,'127.0.0.1',resolve));crossPort=crossServer.address().port;
port=server.address().port;const browser=await engines[engine].launch(),page=await browser.newPage({viewport:{width:1100,height:850}});
page.on('requestfailed',r=>events.push({type:'failed',url:r.url(),error:r.failure()}));page.on('console',m=>events.push({type:'console',text:m.text()}));
try{
 await page.goto(`http://127.0.0.1:${port}${new URL(url).pathname}${new URL(url).search}`);
 const settled=async(id=-1)=>{await page.waitForFunction(id=>window.demoError||(window.demoState?.settled&&window.demoState.requestId>id),id,{timeout:120000});assert.equal(await page.evaluate(()=>window.demoError),undefined);return page.evaluate(()=>window.demoState);};
 initial=await settled();await page.selectOption('#margin','0');initial=await settled(initial.requestId);assert.notEqual(await page.locator('#quality').inputValue(),'lossless');
 await page.selectOption('#quality','lossless');
 const waitFor=async predicate=>{const deadline=Date.now()+15000;while(!predicate()){if(Date.now()>deadline)throw Error('HTTP cancellation deadline');await new Promise(resolve=>setTimeout(resolve,25));}};
 await waitFor(()=>requests.some(r=>r.method==='GET'));
 await page.evaluate(()=>{document.querySelector('#plus').click();document.querySelector('#plus').click();});
 afterZoom=await page.evaluate(()=>({state:window.demoState,error:window.demoError}));
 await waitFor(()=>requests.some(r=>r.method==='GET'&&r.closed&&!r.finished));
 // A second view at the new level must also be cancellable.
 await waitFor(()=>new Set(requests.filter(r=>r.method==='GET').map(r=>r.url)).size>=2);
 const id=await page.evaluate(()=>window.demoState.requestId);
 await page.selectOption('#quality',initial.stats.selection.split(':')[1]);
 const final=await settled(id);
 await waitFor(()=>requests.filter(r=>r.method==='GET').every(r=>r.closed));
 assert.ok(requests.filter(r=>r.method==='GET'&&r.closed&&!r.finished).length>=2);
 assert.equal(final.stats.inflight,0);
 assert.ok(final.stats.cacheBytes>0);
 writeFileSync(resolve(out,'results.json'),JSON.stringify({engine,requests,initial,final},null,2));
 console.log('Rapid zoom and quality changes abort real unfinished HTTP bodies; cached return settles PASS');
}catch(error){writeFileSync(resolve(out,'failure.json'),JSON.stringify({engine,error:String(error),requests,events,initial,afterZoom,current:await page.evaluate(()=>({state:window.demoState,error:window.demoError})).catch(()=>null)},null,2));throw error;}finally{await browser.close();server.closeAllConnections();crossServer.closeAllConnections();await Promise.all([new Promise(resolve=>server.close(resolve)),new Promise(resolve=>crossServer.close(resolve))]);}
