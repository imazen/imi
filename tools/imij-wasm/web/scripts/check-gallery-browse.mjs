// Gallery browser check on a generated metadata-only catalog.
// Serves an unpacked explorer package locally, replaces image/gallery.json with the
// real entries plus N synthetic "Fixture" entries (no thumbnails, no image data), holds
// all WASM responses, and exercises browse/search/filter/sort/paging/keyboard/early
// selection. Synthetic entries are not gallery images and never reach a public catalog.
// usage: check-gallery-browse.mjs WEB PACKAGE_DIR FRESH_OUTPUT [FIXTURE_COUNT=120]
import {createServer} from 'node:http';
import {mkdirSync,readFileSync,statSync,writeFileSync} from 'node:fs';
import {extname,join,normalize,resolve} from 'node:path';
import {pathToFileURL} from 'node:url';
const [web,pkg,out,countArg='120']=process.argv.slice(2);
if(!out)throw Error('usage: check-gallery-browse WEB PACKAGE_DIR FRESH_OUTPUT [FIXTURE_COUNT]');
const fixtureCount=+countArg;const remote=/^https?:\/\//.test(pkg);
mkdirSync(out,{recursive:false});
const types={'.html':'text/html','.js':'text/javascript','.json':'application/json','.css':'text/css','.wasm':'application/wasm'};
const server=createServer((req,res)=>{
 const path=normalize(decodeURIComponent(new URL(req.url,'http://x').pathname)).replace(/^(\.\.[/\\])+/,'');
 let file=join(pkg,path),body;try{if(statSync(file).isDirectory())file=join(file,'index.html');body=readFileSync(file);}catch{res.writeHead(404);res.end();console.error('local 404',req.url);return;}
 res.writeHead(200,{'content-type':types[extname(file)]||'application/octet-stream','cache-control':'no-store'});res.end(body);
});
if(!remote)await new Promise(r=>server.listen(0,'127.0.0.1',r));
const base=remote?pkg.replace(/\/$/,''):`http://127.0.0.1:${server.address().port}`;
const real=remote?await (await fetch(new URL('image/gallery.json',pkg.endsWith('/')?pkg:pkg+'/'))).json():JSON.parse(readFileSync(join(pkg,'image/gallery.json'),'utf8'));
const hosts=['example-a.test','example-b.test','example-c.test'];
const fixtures=Array.from({length:fixtureCount},(_,i)=>({id:`fixture-${i}`,title:`Fixture ${String(i).padStart(3,'0')} ${i%7===0?'Ünïcode café':'panorama'}`,width:4000+i*50,height:3000+i*13,credit:`Credit ${i%5}`,sourceUrl:`https://${hosts[i%3]}/item/${i}`,thumbnail:null,tags:i%10===0?['glacier']:[],collection:i%4===0?'Alpha':'Beta'}));
const gallery={datasets:[...real.datasets,...fixtures]};
// Independent reimplementation of the documented search rule: every word is a substring of title/credit/tags/collection.
const fold=v=>String(v||'').normalize('NFD').replace(/\p{Diacritic}/gu,'').toLowerCase();
const expectedMatches=words=>gallery.datasets.filter(d=>words.every(w=>fold([d.title,d.credit,...(d.tags||[]),d.collection].join(' ')).includes(w))).length;
const {chromium}=await import(pathToFileURL(resolve(web,'node_modules/playwright-core/index.mjs')).href);
const browser=await chromium.launch({args:['--disable-web-security']});
const checks=[];const check=(name,ok,detail)=>{checks.push({name,ok:!!ok,detail});if(!ok)throw Error(`FAILED ${name}: ${JSON.stringify(detail)}`);};
try{
 for(const [label,viewport] of [['desktop',{width:1100,height:900}],['mobile',{width:390,height:800}]]){
  const page=await browser.newPage({viewport}),errors=[];page.on('pageerror',e=>errors.push(String(e)));
  await page.route('**/image/gallery.json',route=>route.fulfill({contentType:'application/json',body:JSON.stringify(gallery)}));
  let release;const held=new Promise(r=>{release=r;});await page.route('**/*.wasm',async route=>{await held;await route.continue();});
  await page.goto(`${base}/gigapixel/index.html`,{waitUntil:'domcontentloaded'});
  await page.locator('#gallery-browse:not([disabled])').waitFor({timeout:30000});
  const total=real.datasets.length+fixtureCount;
  check(`${label}: the strip lists every scene once`,(await page.locator('#gallery-items .gallery-card').count())===real.datasets.length+fixtureCount,await page.locator('#gallery-items .gallery-card').count());
  check(`${label}: browse label counts catalog`,(await page.locator('#gallery-browse').textContent()).includes(String(total)),await page.locator('#gallery-browse').textContent());
  check(`${label}: decoder not ready`,!(await page.evaluate(()=>window.demoState?.ready||false)),null);
  await page.locator('#gallery-browse').click();
  check(`${label}: native dialog open`,await page.evaluate(()=>document.getElementById('gallery-dialog').open),null);
  check(`${label}: search focused`,await page.evaluate(()=>document.activeElement?.id==='gallery-search'),await page.evaluate(()=>document.activeElement?.id));
  const shown=()=>page.locator('#gallery-results .gallery-card').count();
  check(`${label}: first page bounded to 24`,(await shown())===24,await shown());
  check(`${label}: no horizontal overflow`,await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth+1&&document.getElementById('gallery-dialog').scrollWidth<=document.getElementById('gallery-dialog').clientWidth+1),null);
  await page.screenshot({path:resolve(out,`${label}-dialog.png`)});
  // Paging.
  const pages=Math.ceil(total/24);
  check(`${label}: page label`,(await page.locator('#gallery-page').textContent())===`1 / ${pages}`,await page.locator('#gallery-page').textContent());
  check(`${label}: previous disabled`,await page.locator('#gallery-previous').isDisabled(),null);
  await page.locator('#gallery-next').click();
  check(`${label}: page two`,(await page.locator('#gallery-page').textContent())===`2 / ${pages}`&&(await shown())===24,null);
  await page.locator('#gallery-previous').click();
  // Search: accent-insensitive, every word, tags, collection, credit.
  await page.fill('#gallery-search','cafe');
  const cafe=expectedMatches(['cafe']);
  check(`${label}: accent-insensitive search`,(await page.locator('#gallery-count').textContent()).startsWith(`${cafe} image`),await page.locator('#gallery-count').textContent());
  await page.fill('#gallery-search','glacier');
  check(`${label}: tag search`,(await page.locator('#gallery-count').textContent()).startsWith(`${expectedMatches(['glacier'])} image`),await page.locator('#gallery-count').textContent());
  await page.fill('#gallery-search','alpha credit 3');
  // Words are independent substrings of title/credit/tags/collection (all must match).
  const expected=expectedMatches(['alpha','credit','3']);
  check(`${label}: multi-word collection+credit search`,(await shown())===Math.min(24,expected),{expected,shown:await shown()});
  await page.fill('#gallery-search','zzzz-no-match');
  check(`${label}: empty state`,(await shown())===0&&(await page.locator('#gallery-count').textContent()).startsWith('No images match')&&await page.locator('#gallery-next').isDisabled(),null);
  await page.fill('#gallery-search','');
  // Source filter and sort.
  await page.selectOption('#gallery-source','example-b.test');
  const onB=gallery.datasets.filter(d=>new URL(d.sourceUrl).hostname==='example-b.test').length;
  check(`${label}: source filter`,(await page.locator('#gallery-count').textContent()).startsWith(`${onB} image`),await page.locator('#gallery-count').textContent());
  await page.selectOption('#gallery-source','');
  await page.selectOption('#gallery-sort','largest');
  const first=await page.locator('#gallery-results .gallery-card strong').first().textContent();
  const largest=[...gallery.datasets].sort((a,b)=>b.width*b.height-a.width*a.height)[0].title.replace(/ · [\d.]+ gigapixels$/,'');
  check(`${label}: largest first`,first===largest,{first,largest});
  await page.selectOption('#gallery-sort','title');
  const titles=await page.locator('#gallery-results .gallery-card strong').allTextContents();
  check(`${label}: A–Z`,titles.every((t,i)=>!i||titles[i-1].localeCompare(t)<=0),titles.slice(0,4));
  await page.selectOption('#gallery-sort','featured');
  // Escape closes the native dialog and returns focus to the opener.
  await page.keyboard.press('Escape');
  check(`${label}: Escape closes`,!(await page.evaluate(()=>document.getElementById('gallery-dialog').open)),null);
  check(`${label}: focus returns to browse`,await page.evaluate(()=>document.activeElement?.id==='gallery-browse'),await page.evaluate(()=>document.activeElement?.id));
  // Keyboard: open, search, Tab to a result and choose it with Enter, before WASM exists.
  await page.keyboard.press('Enter');
  check(`${label}: Enter opens dialog`,await page.evaluate(()=>document.getElementById('gallery-dialog').open),null);
  await page.fill('#gallery-search','webb');
  await page.locator('#gallery-results .gallery-card[data-scene="webb"]').focus();
  await page.keyboard.press('Enter');
  check(`${label}: choosing closes dialog`,!(await page.evaluate(()=>document.getElementById('gallery-dialog').open)),null);
  check(`${label}: selection pressed in the strip`,(await page.locator('#gallery-items .gallery-card[aria-pressed=true]').getAttribute('data-scene'))==='webb',null);
  const status=await page.locator('#gallery-status').textContent();
  check(`${label}: pending status`,/decoder still loading/.test(status),status);
  // Re-rendering the gallery (opening, filtering) must not reset the pending selection.
  await page.locator('#gallery-browse').click();await page.fill('#gallery-search','fixture 00');
  // Chromium's first Escape in a non-empty search field clears it; the second closes the dialog.
  await page.keyboard.press('Escape');if(await page.evaluate(()=>document.getElementById('gallery-dialog').open))await page.keyboard.press('Escape');
  check(`${label}: Escape closes after filtering`,!(await page.evaluate(()=>document.getElementById('gallery-dialog').open)),null);
  check(`${label}: pending selection survives dialog use`,(await page.locator('#gallery-items .gallery-card[aria-pressed=true]').getAttribute('data-scene'))==='webb',null);
  release();
  await page.waitForFunction(()=>window.demoError||window.demoState?.settled,null,{timeout:180000});
  const state=await page.evaluate(()=>({error:window.demoError,image:window.demoState.image.id,quality:window.demoState.representation?.quality,scene:document.getElementById('scene').value}));
  check(`${label}: held WASM released, webb loaded at default quality`,!state.error&&state.image==='webb'&&state.quality==='webmap'&&state.scene==='webb',state);
  // After ready, picking another scene via the gallery still works.
  await page.locator('#gallery-browse').click();await page.locator('#gallery-results .gallery-card[data-scene="earth"]').click();
  await page.waitForFunction(()=>window.demoState?.image.id==='earth'&&window.demoState.settled,null,{timeout:180000});
  check(`${label}: post-ready selection`,(await page.locator('#scene').inputValue())==='earth',null);
  await page.screenshot({path:resolve(out,`${label}-viewer.png`),fullPage:true});
  check(`${label}: no page errors`,!errors.length,errors);
  await page.close();
 }
 writeFileSync(resolve(out,'results.json'),JSON.stringify({browser:browser.version(),fixtureCount,note:'synthetic Fixture entries are metadata-only and not gallery images',checks},null,2),{flag:'wx'});
 console.log(`gallery browse: ${checks.length} checks PASS`);
}finally{await browser.close();server.close();}
