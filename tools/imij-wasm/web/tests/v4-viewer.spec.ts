// Actual explorer worker/coordinator/viewport path with native-produced V4 assets.
import {test,expect} from '@playwright/test';
import {PORTS} from '../playwright.config';
// The code-value canvas gate is 1:1. Browser resampling at other display
// densities is covered separately by the existing density/viewer checks.
test.use({deviceScaleFactor:1});
test('V4 explorer navigates indexed images, archive entries and pyramid levels',async({page})=>{
  if(!process.env.IMIJ_V4_VIEWER)throw Error('IMIJ_V4_VIEWER with prepared synthetic catalog required');
  await page.setViewportSize({width:800,height:700});
  const native=process.env.IMIJ_NATIVE_PYRAMID;
  const packaged=Boolean(process.env.IMIJ_SITE_STAGE);
  if(packaged&&!native)throw Error('The staged Explorer gate requires a native pyramid');
  const wasmRequests:string[]=[];
  const lanHost=process.env.IMIJ_NATIVE_VIEWER_LAN_HOST;
  if(lanHost&&!native)throw Error('The LAN gate requires a native pyramid');
  const requests=[];
  page.on('request',r=>{
    const path=new URL(r.url()).pathname;
    if(path.startsWith('/image/')||(packaged&&path.startsWith('/d/')))requests.push({path,method:r.method(),range:r.headers().range});
    if(path.endsWith('imij_bg.wasm'))wasmRequests.push(path);
  });
  const errors=[];page.on('pageerror',e=>errors.push(e.message));
  const viewerUrl=`http://${lanHost||'127.0.0.1'}:${PORTS.isolated}/gigapixel/`;
  let opening,releaseHead;
  if(native==='single'){
    let entered;
    opening=new Promise(resolve=>{entered=resolve;});
    const held=new Promise(resolve=>{releaseHead=resolve;});
    let first=true;
    await page.route(packaged?'**/d/**/*.imi':'**/image/*.imi',async route=>{
      if(first&&route.request().method()==='HEAD'){
        first=false;
        entered();
        await held;
      }
      await route.continue();
    });
  }
  try{
    await page.goto(lanHost?viewerUrl:viewerUrl+'?verify=1');
    if(opening){
      await opening;
      const previous=await page.evaluate(()=>window.demoState.requestId);
      await page.setViewportSize({width:816,height:700});
      await page.waitForFunction(previous=>window.demoState?.requestId>previous,previous);
    }
  }finally{
    releaseHead?.();
  }
  if(lanHost)expect(await page.evaluate(()=>typeof crypto.subtle)).toBe('undefined');
  async function settled(previous=-1,quality=null){
    const result=await page.waitForFunction(({previous,quality})=>{
      if(window.demoError)return {error:window.demoError};
      const state=window.demoState;
      return state?.settled&&state.requestId>previous&&(!quality||state.representation?.quality===quality)?state:false;
    },{previous,quality});
    const state=await result.jsonValue();expect(state.error).toBeFalsy();return state;
  }
  async function change(action){const before=await page.evaluate(()=>window.demoState.requestId);await action();return settled(before);}
  const initial=await settled();
  if(native)expect(initial.viewSetup.source.nativeViews).toBe(3);
  await page.locator('details.progress-panel > summary').click();
  await change(()=>page.selectOption('#margin','0'));
  await change(()=>page.check('#redecode'));
  await change(()=>page.selectOption('#workers','2'));
  const cells=[];
  for(const quality of ['lossless','lossy']){
    await change(()=>page.locator(`#overlay-quality [data-quality="${quality}"]`).click());
    for(const level of ['full','half','quarter']){
      const state=await change(()=>page.selectOption('#resolution',level,{force:true}));
      expect(state.representation).toMatchObject({scene:'v4-pattern',quality,level});
      if(native)expect(state.viewSetup.source.nativeViews).toBe(3);
      expect(state.viewDecodedPixels).toBe(state.plannedPixels);expect(state.plannedPixels).toBeGreaterThan(0);
      if(!lanHost){
        const hashes=await page.evaluate(()=>window.demoTileHashes);
        expect(Object.keys(hashes).length).toBeGreaterThan(0);
      }
      cells.push({quality,level,pixels:state.plannedPixels});
    }
  }
  await change(()=>page.locator('#overlay-quality [data-quality="lossless"]').click());
  await change(()=>page.selectOption('#resolution','full',{force:true}));
  for(let n=0;n<12&&await page.evaluate(()=>window.demoState.view.scale<1);n++)await change(()=>page.locator('#plus').click());
  expect(await page.evaluate(()=>window.demoState.view.scale)).toBe(1);
  // Align at integer world/pixel coordinates using existing trusted pan controls.
  await change(()=>page.locator('#view').press('ArrowRight'));
  const exact=await page.evaluate(async()=>{
    await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
    const c=document.querySelector('#view'),ctx=c.getContext('2d'),state=window.demoState;
    const raw=new Uint8Array(await(await fetch('/image/source.rgb')).arrayBuffer());
    const b={x:state.view.x-c.clientWidth/2,y:state.view.y-c.clientHeight/2};
    let checked=0;
    for(const [x,y] of [[256,256],[512,256],[768,256],[256,384],[512,384],[768,384]]){
      const dx=Math.round(x-b.x),dy=Math.round(y-b.y);if(dx<0||dy<0||dx>=c.width||dy>=c.height)continue;
      const got=ctx.getImageData(dx,dy,1,1).data,at=(y*1033+x)*3;
      if(got[0]!==raw[at]||got[1]!==raw[at+1]||got[2]!==raw[at+2]||got[3]!==255)throw Error(`visible pixels ${x},${y}: ${got} vs ${raw.subarray(at,at+3)}`);
      checked++;
    }
    return checked;
  });
  expect(exact).toBeGreaterThan(0);
  // Supersede work through the real page controls and require a settled final owner.
  const previous=await page.evaluate(()=>window.demoState.requestId);
  await page.locator('#overlay-quality [data-quality="lossy"]').click();await page.locator('#overlay-quality [data-quality="lossless"]').click();
  const last=await settled(previous,'lossless');expect(last.representation.quality).toBe('lossless');
  expect(errors).toEqual([]);expect(cells).toHaveLength(6);
  if(packaged){
    expect(wasmRequests.length).toBeGreaterThan(0);
    expect(wasmRequests.every(path=>path.includes('/dist/explorer/'))).toBe(true);
  }
  if(native){
    const assets=requests.filter(r=>!['/image/image.json','/image/gallery.json','/image/source.rgb'].includes(r.path));
    expect(assets.some(r=>r.range?.startsWith('bytes='))).toBe(true);
    if(native==='split'){
      const partPath=packaged?/\/d\/[0-9a-f]{32}\/p\/\d{6}\.bin$/:/\/(lossless|lossy)-parts\/\d{6}\.bin$/;
      expect(assets.every(r=>partPath.test(r.path)&&r.method==='GET'&&r.range)).toBe(true);
      expect(new Set(assets.map(r=>r.path)).size).toBeGreaterThan(2);
    }else{
      expect(assets.every(r=>/\/(lossless|lossy)\.imi$/.test(r.path)&&(r.method==='HEAD'||r.range))).toBe(true);
      expect(assets.filter(r=>r.method==='HEAD')).toHaveLength(2);
    }
    console.log(JSON.stringify({native,assetRequests:assets.length,heads:assets.filter(r=>r.method==='HEAD').length,
      rangeGets:assets.filter(r=>r.range).length,physicalUrls:new Set(assets.map(r=>r.path)).size}));
  }
});

if(process.env.IMIJ_NATIVE_PYRAMID)test('visible V4 ranges start while the preceding view awaits decode delivery',async({page})=>{
  // Hold a real decoder's completion at delivery to the coordinator. The two quality
  // representations have independent range resources, so the second range
  // cannot be a retry or an already cached read from the preceding view.
  const hold=Promise.withResolvers<void>();
  let armed=false;
  await page.route('**/gigapixel/worker.js*',async route=>{
    const response=await route.fetch(),source=await response.text();
    await route.fulfill({response,body:`
      import {RegionPool as GatePool} from './region-pool.js';
      const gateOriginalRegion=GatePool.prototype.region;
      GatePool.prototype.region=async function(...args){
        const result=await gateOriginalRegion.apply(this,args);
        if(result.decodedPixels)await fetch(new URL('./decode-result-gate',location.href));
        return result;
      };
      `+source});
  });
  await page.route('**/gigapixel/decode-result-gate',async route=>{
    if(armed)await hold.promise;
    await route.fulfill({body:''});
  });
  const url=`http://127.0.0.1:${PORTS.isolated}/gigapixel/`;
  const settled=async (previous=0)=>{
    await page.waitForFunction(id=>window.demoError||window.demoState?.settled&&window.demoState.requestId>id,previous);
    const state=await page.evaluate(()=>({error:window.demoError,view:window.demoState}));
    expect(state.error).toBeFalsy();
    return state.view;
  };
  await page.goto(url);
  const initial=await settled();
  const other=await page.evaluate(current=>
    [...document.querySelectorAll('#quality option')].map(option=>(option as HTMLOptionElement).value)
      .find(value=>value!==current),initial.representation.quality);
  expect(other).toBeTruthy();
  // Cold quality change uses a different native file and a real 206 response.
  // Re-decode the initial quality to put a job behind the completion gate.
  try{
    armed=true;
    const entered=page.waitForRequest('**/gigapixel/decode-result-gate',{timeout:5000});
    await page.locator('#redecode').evaluate(node=>{(node as HTMLInputElement).checked=true;node.dispatchEvent(new Event('change'));});
    await entered;
    const before=await page.evaluate(()=>window.demoState.requestId);
    const request=page.waitForRequest(req=>req.headers().range&&/\.(imi|bin)$/.test(new URL(req.url()).pathname),
      {timeout:5000});
    await page.selectOption('#quality',other!,{force:true});
    const range=await request;
    expect(range.headers().range).toMatch(/^bytes=/);
    const next=await page.evaluate(()=>window.demoState);
    expect(next.requestId).toBeGreaterThan(before);
    expect(next.settled).toBeFalsy();
    armed=false;hold.resolve();
    expect((await settled(before)).representation.quality).toBe(other);
  }finally{
    armed=false;hold.resolve();
  }
});
