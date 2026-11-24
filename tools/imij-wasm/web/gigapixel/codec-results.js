// Rates use completed trials; compression includes every encoded-file byte.
export function compression(f){const n=f.width*f.height;return {bpp:f.bytes*8/n,ratio:n*3/f.bytes};}
export function rateRange(trials){
 const speeds=trials.map(t=>t.mpxPerSecond);
 if(!speeds.length||speeds.some(v=>!Number.isFinite(v)||v<=0))return null;
 return {min:Math.min(...speeds),max:Math.max(...speeds)};
}
export function rankedRows(fixtures,results,lossless){
 const rows=fixtures.filter(f=>f.lossless===lossless).map(f=>{const r=results.find(r=>r.id===f.id);return {...f,...compression(f),result:r,range:r?.status==='decoded'?rateRange(r.trials):null};});
 for(const row of rows){
  const peers=rows.filter(p=>p.width===row.width&&p.height===row.height);
  row.bppWinner=row.bpp===Math.min(...peers.map(p=>p.bpp));
  row.speedWinner=!!row.range&&row.range.min===Math.max(...peers.filter(p=>p.range).map(p=>p.range.min));
 }
 return rows.sort((a,b)=>(b.range?.min??-Infinity)-(a.range?.min??-Infinity)||a.width-b.width||a.id.localeCompare(b.id));
}

// Six IMI cases: retain the direct decoder and expose a one-worker region control.
export function benchmarkCases(fixtures){
 return fixtures.flatMap(f=>!f.codec.startsWith('imi-')?[f]:[
  {...f,id:f.id+'-whole-1',fileId:f.id,workers:1,path:'whole',label:'IMI · 1 worker · whole image'},
  ...[1,2,4,8,16].map(workers=>({...f,id:`${f.id}-regions-${workers}`,fileId:f.id,workers,path:'regions',label:`IMI · ${workers} worker${workers===1?'':'s'} · indexed regions`}))
 ]);
}
