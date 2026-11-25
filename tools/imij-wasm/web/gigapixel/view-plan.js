export function representationKey(rep){
  if(rep.kind!=='native')return rep.contentId??rep.sha256;
  const key=`${rep.contentId}:${rep.viewId}`;
  return rep.codingProfile===undefined?key:`${key}:${rep.codingProfile}`;
}
export function imageFromFragment(hash) {
  return new URLSearchParams(hash.slice(1)).get('image');
}
export function fragmentForImage(hash, id) {
  const params = new URLSearchParams(hash.slice(1));
  params.set('image', id);
  return `#${params}`;
}
export function startupScene(catalog, sceneId) {
  const defaultId = catalog.defaultScene || 'eso1719';
  return catalog.datasets.find(scene => scene.id === sceneId)
    ?? catalog.datasets.find(scene => scene.id === defaultId)
    ?? catalog.datasets[0];
}
// Geometry is in original-image coordinates; level dimensions may round upward.
export function levelsFor(scene){return scene.levels||[{key:'full',divisor:1},{key:'quarter',divisor:4}];}
export function selectRepresentation(scene,quality,choice,scale){
  const levels=levelsFor(scene);let level=levels.find(l=>l.key===choice);
  if(choice==='auto')level=levels.filter(l=>l.divisor<=1/scale+1e-8).at(-1)||levels[0];
  if(!level)level=levels[0];
  return {level:level.key,divisor:level.divisor,rep:scene.representations[quality][level.key]};
}
export function minScale(scene,choice,width,height){
  // Source detail does not constrain camera geometry. Large original views
  // stream through bounded region jobs instead of allocating a whole raster.
  return Math.min(1,width/scene.width,height/scene.height);
}
export function planRegions(scene,rep,b,margin,scale,{singleTile=false}={}){
  const tw=rep.tileWidth||512,th=rep.tileHeight||256;
  const fx=scene.width/rep.width,fy=scene.height/rep.height;
  const startX=Math.max(0,Math.floor(b.x/fx/tw)-margin)*tw,startY=Math.max(0,Math.floor(b.y/fy/th)-margin)*th;
  const endX=Math.min(rep.width,(Math.ceil((b.x+b.w)/fx/tw)+margin)*tw),endY=Math.min(rep.height,(Math.ceil((b.y+b.h)/fy/th)+margin)*th);
  const tiles=[];
  // Stable storage-sized jobs at 1:1 retain partially visible tiles during
  // a pan. Overview draws keep 2×2 jobs and their existing resampling.
  const stepX=singleTile?tw:Math.max(tw,1024),stepY=singleTile?th:Math.max(th,512);
  for(let gy=Math.floor(startY/stepY)*stepY;gy<endY;gy+=stepY)for(let gx=Math.floor(startX/stepX)*stepX;gx<endX;gx+=stepX){
    const lx=Math.max(startX,gx),ly=Math.max(startY,gy),lw=Math.min(endX,gx+stepX)-lx,lh=Math.min(endY,gy+stepY)-ly;
    if(lw<=0||lh<=0)continue;
    const visible=(lx+lw)*fx>b.x&&lx*fx<b.x+b.w&&(ly+lh)*fy>b.y&&ly*fy<b.y+b.h;
    tiles.push({key:`${representationKey(rep)}:${lx}:${ly}:${lw}:${lh}:${scale}`,scale,visible,rep:representationKey(rep),lx,ly,lw,lh,x:lx*fx,y:ly*fy,width:lw*fx,height:lh*fy});
  }
  return tiles;
}

// Keep the display job intact while allowing its storage-aligned parts to run
// independently after their combined ranges have been fetched.
export function storageRegions(tile,tw=512,th=256){
  const parts=[];
  const right=tile.lx+tile.lw,bottom=tile.ly+tile.lh;
  for(let y=tile.ly;y<bottom;){
    const nextY=Math.min(bottom,(Math.floor(y/th)+1)*th);
    for(let x=tile.lx;x<right;){
      const nextX=Math.min(right,(Math.floor(x/tw)+1)*tw);
      parts.push({lx:x,ly:y,lw:nextX-x,lh:nextY-y});
      x=nextX;
    }
    y=nextY;
  }
  return parts;
}

// Original is a source-isolation mode, including already-cached display pixels.
export function displayRepresentations(scene,quality,choice,target){
  return new Set((choice==='full'?[target]:Object.values(scene.representations[quality])).map(representationKey));
}

// Spread refinement through an already-downloaded batch. HTTP keeps its adjacent
// storage order; the decoder does not need to paint those bytes top to bottom.
export function refinementOrder(tiles){
  const sorted=[...tiles].sort((a,b)=>a.y-b.y||a.x-b.x),out=[],queue=[[0,sorted.length]];
  for(let i=0;i<queue.length;i++){
    const [a,b]=queue[i];if(a===b)continue;const mid=(a+b)>>1;
    out.push(sorted[mid]);if(a<mid)queue.push([a,mid]);if(mid+1<b)queue.push([mid+1,b]);
  }
  return out;
}

// Cover fills the window without letterboxing, cropping the other axis equally.
export function coverView(scene,width,height){return {x:scene.width/2,y:scene.height/2,scale:Math.min(1,Math.max(width/scene.width,height/scene.height))};}
// Snap shared world endpoints, not each tile's width independently. Fractional
// Canvas2D coverage otherwise blends adjacent tiles with the background twice.
export function tileDestination(tile,bounds,scale,density){
 const x=Math.round((tile.x-bounds.x)*scale*density),y=Math.round((tile.y-bounds.y)*scale*density);
 const right=Math.round((tile.x+tile.width-bounds.x)*scale*density),bottom=Math.round((tile.y+tile.height-bounds.y)*scale*density);
 return {x:x/density,y:y/density,width:(right-x)/density,height:(bottom-y)/density};
}

// A quality is offered only when every pyramid level has its own representation:
// the viewer never substitutes another quality's bytes under a missing level.
export function qualityOptions(scene){
  const levels=levelsFor(scene);
  return Object.keys(scene.representations).filter(key=>levels.every(l=>scene.representations[key][l.key])).map(key=>{
    const rep=scene.representations[key][levels[0].key];
    const target=key==='lossless'?Infinity:Number(rep.qualityTarget);
    const chipLabel=key==='lossless'?'Lossless':(rep.qualityLabel||`Q${target}`);
    const label=key==='lossless'?'Lossless':(rep.qualityDescription||(rep.qualityLabel?`${rep.qualityLabel} · target ${target}`:`Lossy · target ${target}`));
    return {key,target,label,chipLabel};
  }).sort((a,b)=>a.target-b.target);
}
export function defaultQuality(options,previous){
  return options.find(o=>o.key===previous)?.key||options.find(o=>o.key==='webmap')?.key||options.find(o=>o.target===70)?.key||options[0].key;
}
