// Compare what the settled 1:1 canvas displays with a direct raster read.
import assert from 'node:assert/strict';
import {chromium,devices,firefox,webkit} from '../node_modules/playwright-core/index.mjs';

const [url,only]=process.argv.slice(2);
const engines={chromium,firefox,webkit};
if(!url||only&&!engines[only])throw Error('usage: check-pan-pixels.mjs EXPLORER_URL [chromium|firefox|webkit]');

async function afterView(page,id){
  await page.waitForFunction(id=>window.demoError||
    window.demoState?.settled&&window.demoState.requestId>id,id,{timeout:45000});
  return page.evaluate(()=>{
    if(window.demoError)throw Error(window.demoError);
    return window.demoState.requestId;
  });
}

async function checkCanvas(page,quality){
  const result=await page.evaluate(async quality=>{
    await new Promise(resolve=>requestAnimationFrame(()=>requestAnimationFrame(resolve)));
    const canvas=document.querySelector('#view'),{width,height}=canvas;
    const shown=canvas.getContext('2d').getImageData(0,0,width,height).data;
    const {view,representation}=window.demoState;
    if(representation.level!=='full'||representation.quality!==quality||
       Math.abs(view.scale*devicePixelRatio-1)>1e-7)throw Error('Expected a settled original 1:1 view');
    const x=-Math.round(canvas.clientWidth/view.scale/2-view.x);
    const y=-Math.round(canvas.clientHeight/view.scale/2-view.y);
    const viewer=document.querySelector('script[src*="/gigapixel/viewer.js"]').src;
    const catalogUrl=new URL('../image/image.json',viewer);
    const catalog=await(await fetch(catalogUrl)).json();
    const rep=catalog.datasets.find(s=>s.id===representation.scene).representations[quality].full;
    const [{HttpRangeSource},{openNativePyramid},{TileIndex},m]=await Promise.all([
      import(new URL('range-source.js',viewer)),import(new URL('native-pyramid.js',viewer)),
      import(new URL('tile-index.js',viewer)),import(new URL('../dist/pkg-simd/imij.js',viewer)),
    ]);
    await m.default();
    const source=HttpRangeSource.openSegmented(new URL(rep.parts.base,catalogUrl).href,rep.encodedBytes,rep.parts,
      undefined,{parallel:8});
    const ensure=spans=>source.ensureSpans(spans);
    const expected=new Uint8Array(width*height*3);
    if(rep.kind==='native'){
      const planner=await openNativePyramid(source,m.RangePyramid,ensure);
      const reader=new m.RangePyramid(rep.encodedBytes,source.read);
      try{
        for(let row=0;row<height;row+=512){
          const lh=Math.min(512,height-row),rect={lx:x,ly:y+row,lw:width,lh};
          await ensure(await planner.plan(rep,rect,ensure));
          expected.set(reader.decodeRegion(BigInt(rep.viewId),x,y+row,width,lh),row*width*3);
        }
      }finally{reader.free();planner.free();}
    }else{
      const indexBytes=await source.loadAux(new URL(rep.index.url,catalogUrl),rep.index,false,undefined,m.xxh3_128);
      const index=new TileIndex(indexBytes,rep,rep.encodedBytes);
      const entries=[];
      for(let ty=Math.floor(y/256)*256;ty<y+height;ty+=256)
        for(let tx=Math.floor(x/512)*512;tx<x+width;tx+=512){
          const span=index.entry(ty/256*Math.ceil(rep.width/512)+tx/512);
          entries.push({...span,tx,ty});
        }
      await ensure(entries);
      for(const {start,end,tx,ty} of entries){
        const tile=m.decodeSealedTile(source.read(start,end-start,false));
        const left=Math.max(x,tx),right=Math.min(x+width,tx+tile.width);
        for(let row=Math.max(y,ty);row<Math.min(y+height,ty+tile.height);row++){
          const from=((row-ty)*tile.width+left-tx)*3,to=((row-y)*width+left-x)*3;
          expected.set(tile.pixels.subarray(from,from+(right-left)*3),to);
        }
      }
    }
    let mismatches=0,first=null;
    for(let p=0;p<width*height;p++){
      if(shown[p*4]!==expected[p*3]||shown[p*4+1]!==expected[p*3+1]||
         shown[p*4+2]!==expected[p*3+2]||shown[p*4+3]!==255){
        mismatches++;
        first??=[p%width,Math.floor(p/width)];
      }
    }
    return {mismatches,first,width,height,view};
  },quality);
  assert.equal(result.mismatches,0,JSON.stringify(result));
}

for(const [name,engine] of Object.entries(engines)){
  if(only&&name!==only)continue;
  const browser=await engine.launch();
  try{
    for(const quality of ['webmap','lossless']){
      const context=await browser.newContext(name==='chromium'?devices['Pixel 7']:{viewport:{width:1440,height:900}});
      try{
        const page=await context.newPage();
        assert((await page.goto(url,{waitUntil:'commit'}))?.ok());
        let id=await afterView(page,0);
        if(quality==='lossless'){await page.selectOption('#quality',quality);id=await afterView(page,id);}
        await page.locator('#zoom').click();
        id=await afterView(page,id);
        await checkCanvas(page,quality);
        await page.locator('#view').focus();
        for(const key of ['ArrowRight','ArrowDown']){
          await page.keyboard.press(key);
          id=await afterView(page,id);
          await checkCanvas(page,quality);
        }
        console.log(name,quality,'three exact 1:1 canvases');
      }finally{await context.close();}
    }
  }finally{await browser.close();}
}
