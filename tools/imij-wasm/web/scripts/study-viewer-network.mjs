import {spawn} from 'node:child_process';
import {appendFile,writeFile} from 'node:fs/promises';
import {createInterface} from 'node:readline';
import {fileURLToPath} from 'node:url';
import {parseArgs} from 'node:util';
import {chromium,devices} from '../node_modules/playwright-core/index.mjs';

const {values:options}=parseArgs({options:{
  url:{type:'string',default:'https://imi.imazen.org/gigapixel/'},
  output:{type:'string'},
  profile:{type:'string',default:'lte'},
  device:{type:'string',default:'Pixel 7'},
  pan:{type:'string'},
  runs:{type:'string',default:'3'},
  port:{type:'string',default:'3480'},
  cdp:{type:'string'},
  'external-link':{type:'boolean'},
  'allow-host':{type:'string',multiple:true,default:[]},
  'rtt-ms':{type:'string'},
  'down-mbps':{type:'string'},
  'up-mbps':{type:'string'},
  help:{type:'boolean'},
}});
if(options.help||!options.output){
  console.log(`Usage: just web-network-study --output RUN.jsonl [--url URL] [--profile lte]
Profiles: direct, wifi, lte, weak-lte. Override with --rtt-ms, --down-mbps, --up-mbps.
Desktop simulation defaults to --device 'Pixel 7'. Use --device desktop for a desktop viewport.
Use --pan webmap or --pan lossless to time cold and returning 1:1 pans after startup.
Use --cdp http://127.0.0.1:PORT --external-link to measure Android Chrome on a prepared link.`);
  process.exit(options.help?0:2);
}
const url=new URL(options.url);
const runs=Number(options.runs);
const device=options.device==='desktop'?{}:devices[options.device];
if(url.protocol!=='https:'||!['direct','wifi','lte','weak-lte'].includes(options.profile)||
    !Number.isInteger(runs)||runs<=0||!device||
    (options.pan&&!['lossless','webmap'].includes(options.pan)))throw Error('Expected an HTTPS URL, known profile, device, pan quality and positive run count');
if(options.cdp&&options.profile!=='direct'&&!options['external-link'])
  throw Error('A CDP browser needs an externally configured link or --profile direct');

const afterView=(page,id)=>page.waitForFunction(id=>window.demoError||
  window.demoState?.settled&&window.demoState.requestId>id,id,{timeout:90000});

async function panStudy(page,quality,onPan){
  await page.evaluate(()=>{
    // Clock the actual viewer events, independently of automation polling.
    for(const name of ['demoState','demoRendered','demoLastTile']){
      let observed=window[name];
      Object.defineProperty(window,name,{configurable:true,get:()=>observed,set:value=>{
        observed=value;
        const pan=window.studyPan;
        if(!pan?.started||pan.settled!==undefined)return;
        const now=performance.now()-pan.started;
        if(name==='demoRendered')pan.cachedDraw??=now;
        if(name==='demoLastTile')pan.firstTile??=now;
        if(name==='demoState'&&value?.settled&&value.requestId>pan.id)pan.settled=now;
      }});
    }
    addEventListener('keydown',event=>{
      if(window.studyPan?.key===event.key)window.studyPan.started=performance.now();
    },true);
  });
  if(await page.locator('#quality').inputValue()!==quality){
    const id=await page.evaluate(()=>{
      const id=window.demoState.requestId;
      window.studyPan={id,started:performance.now()};
      return id;
    });
    await page.selectOption('#quality',quality);
    await afterView(page,id);
    const activation=await page.evaluate(()=>({timing:window.studyPan,timeOrigin:performance.timeOrigin,update:window.demoUpdate,
      level:window.demoState.representation.level}));
    await onPan({kind:'activation',...activation});
  }
  const id=await page.evaluate(()=>window.demoState.requestId);
  await page.locator('#zoom').click();
  await afterView(page,id);
  await page.locator('#view').focus();
  const keys=['ArrowRight','ArrowRight','ArrowLeft','ArrowLeft','ArrowRight','ArrowDown','ArrowDown','ArrowUp'];
  for(let step=0;step<keys.length;step++){
    const key=keys[step];
    const {id,previous}=await page.evaluate(key=>{
      if(window.demoError)throw Error(window.demoError);
      const {requestId:id,view:previous}=window.demoState;
      window.studyPan={id,key};
      return {id,previous};
    },key);
    await page.keyboard.press(key);
    await afterView(page,id);
    const sample=await page.evaluate(async()=>{
      if(window.demoError)throw Error(window.demoError);
      await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
      const {view,viewDecodedPixels,plannedPixels,representation,stats}=window.demoState;
      return {timing:window.studyPan,timeOrigin:performance.timeOrigin,view,viewDecodedPixels,plannedPixels,representation,
        paintMs:window.demoPaintMs,update:window.demoUpdate,
        transport:stats?.http?.view,requests:stats?.http?.view?.started,rangeBytes:stats?.viewBytes};
    });
    if(sample.timing.settled===undefined||sample.view.scale!==previous.scale||
       sample.view.x===previous.x&&sample.view.y===previous.y||sample.representation.quality!==quality)
      throw Error('Expected a settled pan at constant zoom and quality');
    await onPan({step,key,...sample});
  }
}

const useProxy=options.profile!=='direct'&&!options['external-link'];
let link,browser;
try{
  if(useProxy){
    const path=fileURLToPath(new URL('network-link.py',import.meta.url));
    const hosts=new Set([url.hostname,'gigapixels.imazen.org','codec-corpus.r2.imazen.org',...options['allow-host']]);
    const args=[path,'--profile',options.profile,'--port',options.port];
    for(const host of hosts)args.push('--allow',host);
    for(const key of ['rtt-ms','down-mbps','up-mbps'])if(options[key]!==undefined)args.push('--'+key,options[key]);
    link=spawn('python3',args,{stdio:['ignore','pipe','inherit']});
    await new Promise((resolve,reject)=>{
      link.once('error',reject);
      link.once('exit',code=>reject(Error(`Network link exited ${code}`)));
      createInterface({input:link.stdout}).once('line',line=>{console.log(line);resolve();});
    });
  }
  browser=options.cdp?await chromium.connectOverCDP(options.cdp):await chromium.launch(useProxy?{
    proxy:{server:`http://127.0.0.1:${options.port}`},args:['--disable-quic'],
  }:{});
  const mode=options.cdp?'connected Chrome':options.device==='desktop'?'desktop Chrome':'desktop Chrome with Android viewport and UA';
  await writeFile(options.output,JSON.stringify({kind:'run',mode,browser:browser.version(),url:url.href,
    profile:options.profile,link:useProxy?'TCP CONNECT':options['external-link']?'external':'direct',options})+'\n',{flag:'wx'});
  for(let index=0;index<runs;index++){
    const context=options.cdp?browser.contexts()[0]:await browser.newContext(device);
    const page=await context.newPage(),transfers=[],pending=new Set();
    const observe=request=>{
      try{if(request.frame()!==page.mainFrame())return;}
      // An isolated local context only has the one page. On an attached
      // browser, leave unattributed worker traffic out of the measurement.
      catch{if(options.cdp)return;}
      const job=(async()=>{
        const [sizes,response]=await Promise.all([request.sizes(),request.response()]);
        transfers.push({url:request.url(),type:request.resourceType(),status:response?.status(),
          contentRange:await response?.headerValue('content-range'),
          bodyBytes:sizes.responseBodySize,headerBytes:sizes.responseHeadersSize,timing:request.timing()});
      })().catch(()=>{});
      pending.add(job);
      void job.finally(()=>pending.delete(job));
    };
    context.on('requestfinished',observe);
    try{
      await page.addInitScript(()=>{
        let startup;
        Object.defineProperty(window,'demoStartup',{configurable:true,get:()=>startup,set:value=>{
          startup=value;
          let first;
          Object.defineProperty(value,'firstTileMs',{configurable:true,enumerable:true,get:()=>first,set:ms=>{
            first=ms;
            window.studyFirstTile=performance.now();
            requestAnimationFrame(()=>requestAnimationFrame(()=>{window.studyPaint=performance.now();}));
          }});
        }});
      });
      const response=await page.goto(url.href,{waitUntil:'commit',timeout:60000});
      if(!response?.ok())throw Error(`Explorer HTTP ${response?.status()}`);
      await page.waitForFunction(()=>window.demoError||window.demoState?.settled,null,{timeout:90000});
      const sample=await page.evaluate(async()=>{
        if(window.demoError)throw Error(window.demoError);
        await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
        return {navigation:performance.getEntriesByType('navigation')[0].toJSON(),
          firstTileMs:window.studyFirstTile,paintMs:window.studyPaint,settledMs:performance.now(),
          hardware:navigator.hardwareConcurrency,memoryGb:navigator.deviceMemory,viewport:window.demoDisplay,
          startup:window.demoStartup,update:window.demoUpdate,view:window.demoState.representation};
      });
      await Promise.allSettled([...pending]);
      await appendFile(options.output,JSON.stringify({kind:'view',index,...sample,transfers})+'\n');
      console.log(`Load ${index+1}  first tile ${sample.firstTileMs.toFixed(0)} ms  paint ${sample.paintMs.toFixed(0)} ms  complete ${sample.settledMs.toFixed(0)} ms`);
      if(options.pan)await panStudy(page,options.pan,async sample=>{
        await Promise.allSettled([...pending]);
        const since=sample.timing.started+sample.timeOrigin;
        const network=transfers.filter(t=>t.timing.startTime>=since);
        await appendFile(options.output,JSON.stringify({kind:'pan',index,...sample,transfers:network})+'\n');
        if(sample.kind==='activation')console.log(`Quality activation ${sample.timing.settled.toFixed(0)} ms`);
        else console.log(`Pan ${sample.key}  complete ${sample.timing.settled.toFixed(0)} ms  decode ${sample.viewDecodedPixels} pixels`);
      });
    }finally{
      context.off('requestfinished',observe);
      if(options.cdp)await page.close();else await context.close();
    }
  }
}finally{
  try{await browser?.close();}finally{link?.kill();}
}
