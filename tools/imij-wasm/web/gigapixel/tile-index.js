// Validate every extent at open; retain the compact sidecar instead of eagerly
// allocating an object and 64-character digest string for every offscreen tile.
const pageEntries=256,pageBytes=8+pageEntries*4;
export class TileIndex {
 constructor(bytes,rep,length){
  const magic=new TextDecoder().decode(bytes.subarray(0,8));
  const compact=rep.index?.format==='offsets-v2';
  if(bytes.length<28||magic!==(compact?'IMITILE2':'IMITILE1'))throw Error('tile index header');
  const v=new DataView(bytes.buffer,bytes.byteOffset,bytes.byteLength);
  const [w,h,tw,th,n]=[8,12,16,20,24].map(o=>v.getUint32(o,true));
  const entryBytes=compact?4*n+8*Math.ceil(n/pageEntries):44*n;
  if(!w||!h||w!==rep.width||h!==rep.height||tw!==512||th!==256||n!==Math.ceil(w/tw)*Math.ceil(h/th)||bytes.length!==28+entryBytes)throw Error('tile index geometry');
  this.bytes=bytes;this.view=v;this.length=n;this.byteLength=bytes.byteLength;this.compact=compact;
  let prior=0;
  for(let i=0;i<n;i++){
   const {start,end}=this.span(i);
   if(!Number.isSafeInteger(start)||!Number.isSafeInteger(end)||start<prior||(compact&&i>0&&start!==prior)||end<=start||end-start>16*1024*1024||end>length)throw Error('tile index extent');
   prior=end;
  }
 }
 span(i){
  if(this.compact){
   const page=28+Math.floor(i/pageEntries)*pageBytes,local=i%pageEntries;
   const base=Number(this.view.getBigUint64(page,true)),at=page+8+local*4;
   return {start:base+(local?this.view.getUint32(at-4,true):0),end:base+this.view.getUint32(at,true)};
  }
  const at=28+i*44,start=Number(this.view.getBigUint64(at,true));
  return {start,end:start+this.view.getUint32(at+8,true)};
 }
 entry(i){
  if(!Number.isSafeInteger(i)||i<0||i>=this.length)throw Error('tile index ordinal');
  const span=this.span(i);
  if(this.compact)return {...span,checksummedV4:true};
  const at=28+i*44;
  const hash=Array.from(this.bytes.subarray(at+12,at+44),x=>x.toString(16).padStart(2,'0')).join('');
  return {...span,hash};
 }
}
