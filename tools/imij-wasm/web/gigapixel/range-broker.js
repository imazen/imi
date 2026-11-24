const superseded=()=>new DOMException('View superseded','AbortError');
// Every waiter belongs to both a source generation and a particular view.
export class RangeBroker {
  constructor(){this.waiters=[];this.running=false;}
  ensure(source,spans,isCurrent,owner=source.owner){
    if(!isCurrent())return Promise.reject(superseded());
    let missing=false;for(const r of spans)if(!source.read(r.start,r.end-r.start))missing=true;
    if(!missing)return Promise.resolve();
    this.prune();if(this.waiters.length>=64)return Promise.reject(Error('Range broker exceeds 64 pending consumers'));
    const epoch=source.epoch;
    return new Promise((resolve,reject)=>{this.waiters.push({source,spans,epoch,isCurrent,owner,resolve,reject});if(!this.running){this.running=true;setTimeout(()=>this.flush(),0);}});
  }
  prune(){
    this.waiters=this.waiters.filter(w=>{if(w.isCurrent()&&w.epoch===w.source.epoch)return true;w.reject(superseded());return false;});
  }
  async flush(){
    try{while(this.waiters.length){
      this.prune();const batch=this.waiters.splice(0);
      const groups=[...new Set(batch.map(w=>w.source))];
      await Promise.all(groups.map(async source=>{
        const waiters=batch.filter(w=>w.source===source);
        const current=()=>waiters.some(w=>w.isCurrent()&&w.epoch===source.epoch);
        try{
          if(!current())throw superseded();await source.fill(false,current,waiters.flatMap(w=>w.spans),waiters[0].owner);
          for(const w of waiters)w.isCurrent()&&w.epoch===source.epoch?w.resolve():w.reject(superseded());
        }catch(error){for(const w of waiters)w.reject(error);}
      }));
    }}finally{this.running=false;}
  }
}
