// Display units and geometry stay separate from exact numeric telemetry.
export function pixels(value, gigapixels=false){
  const giga=gigapixels||value>=1e9;
  return `${(value/(giga?1e9:1e6)).toFixed(giga?3:2)} ${giga?'gigapixels':'megapixels'}`;
}
function phaseFraction(phase){
  if(phase.status==='Finished'&&phase.outcome!=='Cancelled')return 1;
  if(Number.isFinite(phase.fraction))return Math.max(0,Math.min(1,phase.fraction));
  const total=Number(phase.total?.value??phase.total?.count??0);
  return total?Math.max(0,Math.min(1,Number(phase.completed||0)/total)):0;
}
export function progressFraction(root){
  if(root.status==='Finished'&&root.outcome!=='Cancelled')return 1;
  if(Number.isFinite(root.fraction))return Math.max(0,Math.min(1,root.fraction));
  return phaseSegments(root).reduce((sum,p)=>sum+p.length,0);
}
// Pack earned phase progress contiguously. No fixed HTTP/decode allocation:
// each how-far weight/fraction determines its current colored contribution.
export function phaseSegments(root){
  const parts=root?.children||[],weight=p=>Math.max(0,Number(p.weight??1));
  const total=parts.reduce((sum,p)=>sum+weight(p),0);let start=0;
  return parts.map((p,index)=>{const length=total?phaseFraction(p)*weight(p)/total:0,segment={index,name:p.name,start,length};start+=length;return segment;});
}
// Union overlapping wall intervals: concurrent workers/requests count once.
export function intervalUnion(intervals){
  let total=0,end=-Infinity;
  for(const [a,b] of [...intervals].sort((a,b)=>a[0]-b[0])){total+=Math.max(0,b-Math.max(a,end));end=Math.max(end,b);}
  return total;
}
export function compressionPercent(rep,refined=false){
  // Every supplied browser rendering is RGB8; for multi-level native pyramids,
  // measure the selected view's bytes rather than the whole pyramid file size.
  const bytes=(refined&&Number.isFinite(rep.refine?.payloadBytes))
    ?rep.refine.payloadBytes
    :(Number.isFinite(rep.levelBytes)?rep.levelBytes:(Number.isFinite(rep.payloadBytes)?rep.payloadBytes:rep.encodedBytes));
  return (100*bytes/(rep.width*rep.height*3)).toFixed(1).replace(/\.0$/,'');
}
// Scale is CSS pixels per source pixel; the readout is device pixels per pixel.
export function snapZoom(scale,density){return Math.abs(scale*density-1)<=.15+1e-10?1/density:scale;}

export function formatRatioPercent(ratio){
  const pct=ratio*100;
  if(!Number.isFinite(pct)||pct<=0)return '0%';
  if(pct>=10)return `${Math.round(pct)}%`;
  return `${pct.toFixed(1).replace(/\.0$/,'')}%`;
}

export function layerQualityTag(rep,qualityKey){
  if(qualityKey==='lossless')return 'lossless';
  if(Number.isFinite(rep?.calibratedQ))return `q${Math.round(rep.calibratedQ)}`;
  if(Number.isFinite(rep?.qualityTarget))return `q${Math.round(rep.qualityTarget)}`;
  return qualityKey||'lossy';
}

export function refineQualityTag(rep){
  if(Number.isFinite(rep?.refine?.calibratedQ))return `q${Math.round(rep.refine.calibratedQ)}`;
  return 'HQ';
}

// Bottom-right zoom pill:
// - Reduced layers (divisor > 1): "<ratio vs 1x> / <ratio vs layer> <layer quality>"
// - Last layer (divisor === 1): simplified to "<ratio vs 1x> <window loaded quality>"
export function formatZoomReadout({scale,density,divisor=1,qualityKey,rep,windowLoaded=null}){
  const ratio1x=scale*density;
  const pct1x=formatRatioPercent(ratio1x);
  const baseTag=layerQualityTag(rep,qualityKey);
  if(divisor>1){
    const pctLayer=formatRatioPercent(ratio1x*divisor);
    const loading=Boolean(windowLoaded&&!windowLoaded.complete);
    return `${pct1x} / ${pctLayer} ${baseTag}${loading?'…':''}`;
  }
  const refineTag=rep?.refine?.viewId?refineQualityTag(rep):null;
  if(!windowLoaded||windowLoaded.total===0)return `${pct1x} ${baseTag}`;
  const {total,base=0,refined=0,missing=0,refining=false,fallbackTag=null}=windowLoaded;
  const loadedTarget=base+refined;
  if(missing>0){
    if(loadedTarget===0&&fallbackTag)return `${pct1x} ${fallbackTag}→${baseTag}`;
    return `${pct1x} ${baseTag} (${loadedTarget}/${total})`;
  }
  if(refineTag){
    if(refined>=total)return `${pct1x} ${refineTag}`;
    if(refining)return `${pct1x} ${baseTag}→${refineTag}`;
    if(refined>0)return `${pct1x} ${baseTag}/${refineTag}`;
  }
  return `${pct1x} ${baseTag}`;
}

// Throttle byte samples, never a transport start/end transition.
export function cacheNotificationDue(previous,next,elapsed){return elapsed>80||(previous.inflight||0)!==(next.inflight||0)||(previous.activeFills||0)!==(next.activeFills||0);}
