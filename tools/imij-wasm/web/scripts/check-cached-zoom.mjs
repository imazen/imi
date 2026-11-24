// Hold fresh HTTP requests and require previously decoded levels to remain visible.
import {mkdirSync,writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,url,out]=process.argv.slice(2);mkdirSync(out,{recursive:false});
const engines=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href),results=[];
for(const name of ['chromium','firefox','webkit']){
 const browser=await engines[name].launch();let release;try{
  const page=await browser.newPage({viewport:{width:1440,height:1000},deviceScaleFactor:2});let hold=false,blocked=0;
  const gate=new Promise(resolve=>{release=resolve;});await page.route('https://codec-corpus.r2.imazen.org/**',async route=>{if(hold){blocked++;await gate;}await route.continue();});
  await page.goto(url);
  async function settled(id=-1){await page.waitForFunction(id=>window.demoError||(window.demoState?.settled&&window.demoState.requestId>id),id,{timeout:120000});if(await page.evaluate(()=>window.demoError))throw Error(await page.evaluate(()=>window.demoError));return page.evaluate(()=>window.demoState);}
  await settled();const before=await settled();hold=true;
  // At 2x density, quarter CSS zoom requests half-resolution source pixels.
  for(let i=0;i<4;i++)await page.locator('#minus').click();
  await page.waitForFunction(target=>window.demoRendered?.target!==target&&window.demoRendered?.fallbackTiles>0&&window.demoState?.needed>0,before.representation.key);
  const preview=await page.evaluate(()=>({render:window.demoRendered,state:window.demoState,center:[...document.querySelector('#view').getContext('2d').getImageData(document.querySelector('#view').width/2,document.querySelector('#view').height/2,1,1).data]}));
  if(!blocked||preview.render.currentTiles||!preview.render.fallbackTiles||preview.state.settled||preview.center.slice(0,3).every((v,i)=>v===[23,36,31][i]))throw Error('cached preview missing while new resolution is blocked');
  await page.screenshot({path:resolve(out,`${name}-cached-preview.png`)});hold=false;release();const final=await settled(before.requestId);
  await page.waitForFunction(()=>window.demoRendered?.currentTiles>0);
  results.push({engine:name,density:2,blocked,preview,final,render:await page.evaluate(()=>window.demoRendered)});writeFileSync(resolve(out,'results.json'),JSON.stringify(results,null,2));console.log(`${name}: cached tiles downsample while HTTP is blocked, fresh level replaces them PASS`);
 }finally{release?.();await browser.close();}
}
