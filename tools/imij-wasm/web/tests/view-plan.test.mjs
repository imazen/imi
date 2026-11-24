import test from 'node:test';
import assert from 'node:assert/strict';
import {levelsFor,selectRepresentation,minScale,planRegions,displayRepresentations,refinementOrder,qualityOptions,defaultQuality,storageRegions,imageFromFragment,fragmentForImage,startupScene} from '../gigapixel/view-plan.js';
import {gesture,applyGesture} from '../gigapixel/gestures.js';
test('image links preserve other fragment options and round-trip scene ids',()=>{
 assert.equal(imageFromFragment(''),null);
 assert.equal(imageFromFragment('#fullscreen'),null);
 assert.equal(imageFromFragment('#image=bigpine'),'bigpine');
 const hash=fragmentForImage('#fullscreen&image=earth','reef & space');
 assert.equal(imageFromFragment(hash),'reef & space');
 const params=new URLSearchParams(hash.slice(1));
 assert.equal(params.has('fullscreen'),true);
 assert.deepEqual(params.getAll('image'),['reef & space']);
});
test('missing or unknown image links open the catalog default',()=>{
 const catalog={datasets:[{id:'earth'},{id:'milkyway'},{id:'bigpine'}],defaultScene:'milkyway'};
 assert.equal(startupScene(catalog,imageFromFragment('#image=bigpine')).id,'bigpine');
 assert.equal(startupScene(catalog,imageFromFragment('#image=missing')).id,'milkyway');
 assert.equal(startupScene(catalog,imageFromFragment('')).id,'milkyway');
});
const levels=[{key:'full',divisor:1},{key:'half',divisor:2},{key:'quarter',divisor:4},... [8,16,32,64,128].map(divisor=>({key:`level-${divisor}`,divisor}))];
const scene={width:43200,height:43200,levels,representations:{lossless:Object.fromEntries(levels.map(l=>[l.key,{sha256:l.key,width:Math.ceil(43200/l.divisor),height:Math.ceil(43200/l.divisor)}]))}};
test('storage jobs clip an unaligned display rectangle at both tile edges',()=>{
 assert.deepEqual(storageRegions({lx:500,ly:250,lw:20,lh:12}),[
  {lx:500,ly:250,lw:12,lh:6},{lx:512,ly:250,lw:8,lh:6},
  {lx:500,ly:256,lw:12,lh:6},{lx:512,ly:256,lw:8,lh:6},
 ]);
});
test('Original display excludes cached pyramid pixels while Auto can reuse them',()=>{
 const target=scene.representations.lossless.full;
 assert.deepEqual([...displayRepresentations(scene,'lossless','full',target)],['full']);
 assert.deepEqual([...displayRepresentations(scene,'lossless','auto',target)],levels.map(l=>l.key));
});
test('ready-batch refinement spreads rows without changing any job or input order',()=>{
 const input=Array.from({length:16},(_,y)=>({y,x:0,key:String(y)})),before=[...input];
 const ordered=refinementOrder(input);
 assert.deepEqual(input,before);assert.deepEqual(ordered.slice(0,3).map(t=>t.y),[8,4,12]);
 assert.deepEqual(new Set(ordered),new Set(input));assert.equal(ordered.length,input.length);
 assert.deepEqual(refinementOrder([]),[]);assert.deepEqual(refinementOrder([input[0]]),[input[0]]);
});
test('automatic selection advances one half-step at each zoom boundary',()=>{
 for(const l of levels){assert.equal(selectRepresentation(scene,'lossless','auto',1/l.divisor).level,l.key);if(l.divisor>1)assert.equal(selectRepresentation(scene,'lossless','auto',1.1/l.divisor).divisor,l.divisor/2);}
 assert.equal(selectRepresentation(scene,'lossless','quarter',4).divisor,4);
 assert.equal(minScale(scene,'auto',1200,800),800/43200);
 assert.equal(levelsFor({}).length,2);
});
test('cross-group rectangles cover exactly the original group window without overlap',()=>{
 for(const width of [338,1822,43200])for(const height of [264,1056,43200])for(const margin of [0,1,2]){
  const rep={sha256:'image',width,height},s={width,height};const b={x:width/3,y:height/3,w:Math.min(width/2,1450),h:Math.min(height/2,900)};
  const planned=planRegions(s,rep,b,margin,1);let area=0;
  for(let i=0;i<planned.length;i++){
   const a=planned[i];assert(a.lw>0&&a.lh>0&&a.lw<=1024&&a.lh<=512);assert(a.lx>=0&&a.ly>=0&&a.lx+a.lw<=width&&a.ly+a.lh<=height);area+=a.lw*a.lh;
   for(const z of planned.slice(0,i))assert(a.lx+a.lw<=z.lx||z.lx+z.lw<=a.lx||a.ly+a.lh<=z.ly||z.ly+z.lh<=a.ly,'overlapping rectangles');
  }
  const x0=Math.max(0,Math.floor(b.x/512)-margin)*512,y0=Math.max(0,Math.floor(b.y/256)-margin)*256;
  const x1=Math.min(width,(Math.ceil((b.x+b.w)/512)+margin)*512),y1=Math.min(height,(Math.ceil((b.y+b.h)/256)+margin)*256);
  assert.equal(area,(x1-x0)*(y1-y0));
 }
 const p=planRegions({width:4096,height:2048},{sha256:'a',width:4096,height:2048},{x:0,y:0,w:4096,h:2048},0,1);
 assert.equal(p.length,16);assert.equal(p.length*4,64); // Four native groups per job.
});
test('pinch preserves the midpoint world anchor while scaling and translating',()=>{
 const points=new Map([[1,{x:400,y:250}],[2,{x:600,y:250}]]),view={x:1000,y:2000,scale:1};
 const start=gesture(points,view,1000,500);points.set(1,{x:350,y:280});points.set(2,{x:750,y:280});
 const next=applyGesture(start,points,1000,500,.1,4);assert.deepEqual(next,{scale:2,x:975,y:1985});
 assert.equal(next.x+(550-500)/next.scale,start.worldX);assert.equal(next.y+(280-250)/next.scale,start.worldY);
 const clamped=applyGesture(start,points,1000,500,.1,1.5);assert.equal(clamped.scale,1.5);assert.equal(clamped.x+50/1.5,start.worldX);
 points.delete(2);const pan=gesture(points,next,1000,500);points.set(1,{x:390,y:300});const after=applyGesture(pan,points,1000,500,.1,4);assert.equal(after.scale,2);assert.equal(after.x,next.x-20);assert.equal(after.y,next.y-10);
});

test('device density selects source detail without changing CSS zoom geometry',()=>{
 for(const dpr of [1,1.25,2,3]){
  const cssScale=.25,selected=selectRepresentation(scene,'lossless','auto',cssScale*dpr);
  assert(selected.divisor<=1/(cssScale*dpr));
  const minimum=minScale(scene,'auto',1200*dpr,800*dpr)/dpr;
  assert.equal(minimum,800/43200);
 }
 assert.equal(selectRepresentation(scene,'lossless','auto',.25*2).level,'half');
});

test('Original keeps its source while allowing the same full zoom-out as Auto',()=>{
 for(const density of [1,2,3])for(const [width,height] of [[1200,800],[390,844]]){
  const scale=minScale(scene,'auto',width*density,height*density)/density;
  assert.equal(minScale(scene,'full',width*density,height*density)/density,scale);
  assert.equal(selectRepresentation(scene,'lossless','full',scale*density).level,'full');
 }
});

test('a gigapixel original is split into bounded decode regions at overview zoom',()=>{
 const large={width:108199,height:81503},rep={...large,sha256:'original'};
 const jobs=planRegions(large,rep,{x:0,y:0,w:large.width,h:large.height},0,.01);
 assert.equal(jobs.reduce((n,t)=>n+t.lw*t.lh,0),large.width*large.height);
 assert(jobs.every(t=>t.lw<=1024&&t.lh<=512));
 assert(jobs.every(t=>t.rep==='original'&&t.scale===.01));
});

test('initial cover fills the window and shared snapped tile edges meet at fractional zoom',async()=>{
 const {coverView,tileDestination}=await import('../gigapixel/view-plan.js');
 for(const [width,height] of [[1200,800],[390,844],[1920,1080]]){
  const v=coverView(scene,width,height);assert(v.scale*scene.width>=width);assert(v.scale*scene.height>=height);assert.equal(v.x,scene.width/2);assert.equal(v.y,scene.height/2);
 }
 for(const scale of [.00137,.27,.731,1])for(const density of [1,1.25,2,3])for(const offset of [-513.17,0,117.39]){
  const bounds={x:offset,y:offset};
  const left=tileDestination({x:0,y:0,width:1024,height:512},bounds,scale,density);
  const right=tileDestination({x:1024,y:0,width:339,height:512},bounds,scale,density);
  const below=tileDestination({x:0,y:512,width:1024,height:127},bounds,scale,density);
  assert(Math.abs(left.x+left.width-right.x)<1e-9);assert(Math.abs(left.y+left.height-below.y)<1e-9);
  assert.equal(left.width*density,Math.round((1024-offset)*scale*density)-Math.round(-offset*scale*density));
 }
});
test('qualities are offered only when every level exists, ordered by target, with their own labels',()=>{
  const rep=target=>({sha256:`s${target}`,qualityTarget:target});
  const scene={levels:[{key:'full',divisor:1},{key:'half',divisor:2}],representations:{
    lossless:{full:rep(),half:rep()},webmap:{full:rep(70),half:rep(70)},lossy90:{full:rep(90),half:rep(90)},
    lossy50:{full:rep(50.0),half:rep(50)},lossy60:{full:rep(60)}}};
  const options=qualityOptions(scene);
  assert.deepEqual(options.map(o=>o.key),['lossy50','webmap','lossy90','lossless']);
  assert.deepEqual(options.map(o=>o.label),['Lossy · target 50','Lossy · target 70','Lossy · target 90','Lossless']);
  assert.equal(defaultQuality(options,undefined),'webmap');assert.equal(defaultQuality(options,'lossy90'),'lossy90');
  assert.equal(defaultQuality(options,'lossy60'),'webmap');
  assert.equal(selectRepresentation(scene,'lossy90','half',1).rep.sha256,'s90');
});

test('1:1 panning preserves display identities of the overlapping storage tiles',()=>{
 const scene={width:4096,height:2048};
 const rep={...scene,kind:'native',contentId:'pyramid',viewId:'1'};
 const previous=planRegions(scene,rep,{x:500,y:250,w:1082,h:1583},0,1,{singleTile:true});
 const next=planRegions(scene,rep,{x:972,y:250,w:1082,h:1583},0,1,{singleTile:true});
 const held=new Set(previous.map(tile=>tile.key));
 assert(next.some(tile=>held.has(tile.key)));
 assert(next.filter(tile=>!held.has(tile.key)).every(tile=>tile.lx>=2048));
 assert(next.every(tile=>tile.lw<=512&&tile.lh<=256));
});

test('1024x512 native storage tiles align 1:1 jobs to storage tile boundaries and honor custom quality labels',()=>{
 const scene={width:4096,height:2048,levels:[{key:'full',divisor:1}],representations:{
  webmap:{full:{kind:'native',contentId:'p',viewId:'1001',width:4096,height:2048,tileWidth:1024,tileHeight:512,qualityTarget:75,qualityLabel:'Tiered',qualityDescription:'Tiered 1->0.5 JND'}},
  lossless:{full:{kind:'native',contentId:'l',viewId:'1',width:4096,height:2048,tileWidth:512,tileHeight:256}},
 }};
 const tiles=planRegions(scene,scene.representations.webmap.full,{x:500,y:250,w:1024,h:512},0,1,{singleTile:true});
 assert.equal(tiles.length,4);
 assert(tiles.every(t=>t.lw===1024&&t.lh===512&&t.lx%1024===0&&t.ly%512===0));
 const opts=qualityOptions(scene);
 assert.deepEqual(opts.map(o=>o.chipLabel),['Tiered','Lossless']);
 assert.deepEqual(opts.map(o=>o.label),['Tiered 1->0.5 JND','Lossless']);
});
