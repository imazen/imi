import {tileDestination} from './view-plan.js';

// Flatten a large source view into one screen-resolution cache surface rather
// than retaining thousands of tiny ImageBitmaps. Geometry stays in world units.
export function overviewTile(bounds,scale,maxBytes,createCanvas){
  const pixels=Math.max(1,Math.floor(maxBytes/4));
  let width=Math.max(1,Math.round(bounds.w*scale)),height=Math.max(1,Math.round(bounds.h*scale));
  if(width*height>pixels){scale=Math.min(scale*Math.sqrt(pixels/(width*height)),pixels/Math.max(bounds.w,bounds.h));width=Math.max(1,Math.floor(bounds.w*scale));height=Math.max(1,Math.floor(bounds.h*scale));}
  // Unrefined pixels stay transparent so compatible cached pixels remain visible.
  const bitmap=createCanvas(width,height),context=bitmap.getContext('2d',{alpha:true});
  if(!context)throw Error('overview canvas unavailable');
  context.imageSmoothingQuality='high';
  return {bitmap,context,bounds:{...bounds},scale,x:bounds.x,y:bounds.y,width:width===bounds.w*scale?bounds.w:width/scale,height:height===bounds.h*scale?bounds.h:height/scale,bytes:width*height*4};
}
export function addOverviewTile(overview,tile){
  const d=tileDestination(tile,overview.bounds,overview.scale,1);
  try{if(d.width>0&&d.height>0)overview.context.drawImage(tile.bitmap,d.x,d.y,d.width,d.height);}
  finally{tile.bitmap.close();}
}
export function closeDisplayImage(bitmap){
  if(typeof bitmap.close==='function')bitmap.close();
  else {bitmap.width=0;bitmap.height=0;}
}
