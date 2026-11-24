import test from 'node:test';
import assert from 'node:assert/strict';
import {overviewTile,addOverviewTile,closeDisplayImage} from '../gigapixel/overview-cache.js';

function canvas(width,height){const calls=[];return {width,height,calls,getContext:()=>({drawImage(...args){calls.push(args);}})};}
test('an original overview retains screen-sized pixels, not the source raster',()=>{
 const view=overviewTile({x:20,y:40,w:100000,h:80000},.01,64*1024*1024,canvas);
 assert.equal(view.bitmap.width,1000);assert.equal(view.bitmap.height,800);
 assert.equal(view.bytes,3_200_000);assert.equal(view.width,100000);assert.equal(view.height,80000);
});
test('the overview surface respects its pixel budget',()=>{
 const view=overviewTile({x:0,y:0,w:100000,h:100000},1,4*1024*1024,canvas);
 assert(view.bytes<=4*1024*1024);assert.equal(view.width,100000);assert.equal(view.height,100000);
});
test('unfinished original detail is transparent over compatible cached pixels',()=>{
 let options;
 overviewTile({x:0,y:0,w:100,h:100},1,40000,(width,height)=>({width,height,getContext:(_kind,value)=>{options=value;return {};}}));
 assert.equal(options.alpha,true);
});
test('neighboring tiles meet on the shared overview pixel grid and are closed',()=>{
 const view=overviewTile({x:13.5,y:0,w:5000,h:3000},.123,64*1024*1024,canvas);
 let closed=0;
 for(const x of [0,1024])addOverviewTile(view,{bitmap:{close(){closed++;}},x,y:0,width:1024,height:512});
 const [a,b]=view.bitmap.calls;
 assert.equal(a[1]+a[3],b[1]);assert.equal(closed,2);
 closeDisplayImage(view.bitmap);assert.equal(view.bitmap.width,0);assert.equal(view.bitmap.height,0);
});
test('failed drawing still releases the incoming bitmap',()=>{
 let closed=false;
 assert.throws(()=>addOverviewTile({bounds:{x:0,y:0},scale:1,context:{drawImage(){throw Error('draw');}}},{x:0,y:0,width:1,height:1,bitmap:{close(){closed=true;}}}),/draw/);
 assert.equal(closed,true);
});
