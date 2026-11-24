import assert from 'node:assert/strict';
import {chromium,firefox,webkit} from '../node_modules/playwright-core/index.mjs';

const [base,name='chromium',first='bigpine',second='webb']=process.argv.slice(2);
const engine={chromium,firefox,webkit}[name];
if(!base||!engine||first===second)throw Error('usage: check-viewer-links.mjs EXPLORER_URL [ENGINE [FIRST_IMAGE SECOND_IMAGE]]');
const url=new URL(base);
url.searchParams.set('link-test','1');
url.hash=new URLSearchParams({image:first,fullscreen:''}).toString();
const browser=await engine.launch();
try{
  const page=await browser.newPage({viewport:{width:1100,height:900}}),errors=[];
  page.on('pageerror',error=>errors.push(String(error)));
  await page.addInitScript(()=>{
    const NativeWorker=window.Worker;
    window.startupImages=[];
    window.Worker=class extends NativeWorker{
      postMessage(message,...args){
        if(message.type==='startup-view'){
          const params=new URLSearchParams(message.fragment?.slice(1));
          window.startupImages.push(params.get('image'));
        }
        return super.postMessage(message,...args);
      }
    };
  });
  const {promise:held,resolve:release}=Promise.withResolvers();
  const holdWasm=async route=>{await held;await route.continue();};
  await page.route('**/*.wasm',holdWasm);
  try{
    await page.goto(url.href,{waitUntil:'domcontentloaded'});
    await page.waitForFunction(()=>window.demoStartup?.galleryReadyMs!==undefined&&window.startupImages.length>0);
    assert.deepEqual(await page.evaluate(()=>({
      image:window.startupImages[0],
      ready:Boolean(window.demoState?.ready),
      fullscreen:document.querySelector('.viewer').classList.contains('expanded'),
    })),{image:first,ready:false,fullscreen:true});
  }finally{release();}
  async function settled(image){
    const result=await page.waitForFunction(id=>window.demoError||
      (window.demoState?.settled&&window.demoState.image.id===id),image,{timeout:60000});
    assert.equal(await result.jsonValue(),true);
    assert.equal(await page.locator('#scene').inputValue(),image);
  }
  await settled(first);
  await page.keyboard.press('Escape');
  await page.locator('#gallery-toggle').click();
  const escaped=await page.evaluate(id=>CSS.escape(id),second);
  await page.locator(`#gallery-items .gallery-card[data-scene="${escaped}"]`).click();
  await settled(second);
  const shared=new URL(page.url());
  assert.equal(shared.searchParams.get('link-test'),'1');
  const hash=new URLSearchParams(shared.hash.slice(1));
  assert.equal(hash.get('image'),second);
  assert(hash.has('fullscreen'));
  await page.goBack();
  await settled(first);
  await page.goForward();
  await settled(second);
  await page.reload();
  await settled(second);
  assert.deepEqual(errors,[]);
  console.log(`${name}: startup image, gallery sharing, Back/Forward and reload passed`);
}finally{await browser.close();}
