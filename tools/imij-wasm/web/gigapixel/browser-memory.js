// UA memory is a browser estimate of the page and workers, not process RSS.
// A pending GC-backed sample never blocks rendering or launches another sample.
export class BrowserMemory {
  constructor(perf,isolated,onChange,now=()=>Date.now()){
    this.perf=perf;this.onChange=onChange;this.now=now;this.pending=null;
    this.state={status:!isolated?'isolation-required':typeof perf.measureUserAgentSpecificMemory!=='function'?'unsupported':'ready',sample:null};
  }
  sample(){
    if(this.pending)return this.pending;
    if(!['ready','measured','error'].includes(this.state.status))return Promise.resolve(this.state);
    this.state={...this.state,status:'measuring',error:null};this.onChange(this.state);
    this.pending=Promise.resolve().then(()=>this.perf.measureUserAgentSpecificMemory()).then(result=>{
      if(!Number.isFinite(result.bytes)||result.bytes<0)throw Error('Invalid browser memory result');
      this.state={status:'measured',sample:{bytes:result.bytes,breakdown:result.breakdown||[],time:this.now()}};
    }).catch(error=>{this.state={...this.state,status:'error',error:`${error.name}: ${error.message}`};}).finally(()=>{this.pending=null;this.onChange(this.state);});
    return this.pending;
  }
}
