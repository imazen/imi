import {explorerWasm} from './wasm-package.js';

async function readFixture(profile){
  const response=await fetch(profile.url,{priority:'low'});
  if(!response.ok||!response.body)throw Error('Native warmup HTTP response');
  const out=new Uint8Array(profile.bytes),reader=response.body.getReader();
  let offset=0;
  try{
    for(;;){
      const {value,done}=await reader.read();
      if(done)break;
      if(value.byteLength>out.length-offset)throw Error('Native warmup exceeds byte budget');
      out.set(value,offset);
      offset+=value.byteLength;
    }
  }finally{await reader.cancel();}
  if(offset!==out.length)throw Error('Truncated native warmup');
  return out;
}

self.onmessage=async ({data:{variant,profile,length,viewId,verify}})=>{
  const module=new Promise(resolve=>{
    self.onmessage=({data})=>resolve(data.module);
  });
  try{
    const [m,bytes,compiled]=await Promise.all([
      import(explorerWasm(variant,'imij.js')),readFixture(profile),module,
    ]);
    await m.default({module_or_path:compiled});
    if(verify&&m.xxh3_128(bytes)!==profile.xxh3)throw Error('Native warmup byte identity');
    let offset=0;
    const spans=profile.spans.map(({start,end})=>{
      const part=bytes.subarray(offset,offset+end-start);
      offset+=part.length;
      return {start,end,part};
    });
    const reader=new m.RangePyramid(length,(start,n)=>{
      const span=spans.find(s=>s.start<=start&&start+n<=s.end);
      if(!span)throw Error('Native warmup range missing');
      return span.part.subarray(start-span.start,start-span.start+n);
    },undefined,1,verify);
    try{
      const view=BigInt(viewId);
      // Repeated calls exercise tier-up before the first visible regions arrive.
      for(let i=0;i<16;i++)reader.decodeRegion(view,...profile.rect);
    }finally{reader.free();}
    postMessage({ok:true});
  }catch{postMessage({ok:false});}
};
