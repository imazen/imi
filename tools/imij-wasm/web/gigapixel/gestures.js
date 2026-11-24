// Pointer coordinates are CSS pixels in the canvas; world coordinates stay original-sized.
export function gesture(points,view,width,height){
  const a=[...points.values()].slice(0,2),center={x:a.reduce((n,p)=>n+p.x,0)/a.length,y:a.reduce((n,p)=>n+p.y,0)/a.length};
  return {center,distance:a.length===2?Math.hypot(a[1].x-a[0].x,a[1].y-a[0].y):0,scale:view.scale,
    worldX:view.x+(center.x-width/2)/view.scale,worldY:view.y+(center.y-height/2)/view.scale};
}
export function applyGesture(start,points,width,height,minimum,maximum,snap=scale=>scale){
  const next=gesture(points,{x:0,y:0,scale:1},width,height);
  const scale=Math.max(minimum,Math.min(maximum,snap(start.distance?start.scale*next.distance/start.distance:start.scale)));
  return {scale,x:start.worldX-(next.center.x-width/2)/scale,y:start.worldY-(next.center.y-height/2)/scale};
}
