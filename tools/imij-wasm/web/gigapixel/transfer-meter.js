// Per-selection peaks use real body-byte counter deltas over >=250 ms windows.
// Returning to a cached selection resets the sample baseline, not its history.
export class TransferMeter {
  constructor(){this.records=new Map();this.selection=null;this.samples=[];}
  select(selection,bytes,now){
    this.selection=selection;this.samples=[{time:now,bytes}];
    if(!this.records.has(selection))this.records.set(selection,{mibPerSecond:0,intervalMs:0});
  }
  sample(bytes,now){
    if(this.selection===null)return {rate:0,elapsed:0,peak:{mibPerSecond:0,intervalMs:0}};
    this.samples.push({time:now,bytes});
    while(this.samples.length>1&&this.samples[1].time<now-1500)this.samples.shift();
    const first=this.samples[0],elapsed=now-first.time;
    const rate=elapsed>0?Math.max(0,bytes-first.bytes)/elapsed*1000/1024/1024:0;
    let peak=this.records.get(this.selection);
    if(elapsed>=250&&rate>peak.mibPerSecond){peak={mibPerSecond:rate,intervalMs:elapsed};this.records.set(this.selection,peak);}
    return {rate,elapsed,peak};
  }
}
