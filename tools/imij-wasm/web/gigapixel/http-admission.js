// One coordinator-wide admission budget, including HEAD, index and part reads.
// Waiting here is application work, not a request submitted to the browser.
export class HttpAdmission {
  constructor(limit=8,maxQueued=32){this.limit=limit;this.maxQueued=maxQueued;this.active=0;this.background=0;this.waiters=[];}
  acquire(background,signal){
    if(signal?.aborted)return Promise.reject(signal.reason);
    if(this.active<this.limit&&(!background||this.background===0)&&!this.waiters.length)return this.admit(background);
    return new Promise((resolve,reject)=>{
      if(this.waiters.length>=this.maxQueued)return reject(Error('HTTP admission queue exceeds bounded demand'));
      const waiter={background,signal,resolve,reject};
      waiter.abort=()=>{const i=this.waiters.indexOf(waiter);if(i>=0)this.waiters.splice(i,1);reject(signal.reason);this.drain();};
      signal?.addEventListener('abort',waiter.abort,{once:true});
      this.waiters.push(waiter);this.drain();
    });
  }
  async run(signal,task){
    const release=await this.acquire(false,signal);
    try{signal?.throwIfAborted();return await task();}
    finally{release();}
  }
  admit(background){
    this.active++;if(background)this.background++;let released=false;
    return ()=>{if(released)return;released=true;this.active--;if(background)this.background--;this.drain();};
  }
  drain(){
    while(this.active<this.limit){
      let i=this.waiters.findIndex(w=>!w.background);
      if(i<0&&this.background===0)i=this.waiters.findIndex(w=>w.background);
      if(i<0)return;
      const w=this.waiters.splice(i,1)[0];w.signal?.removeEventListener('abort',w.abort);
      w.resolve(this.admit(w.background));
    }
  }
}
export const httpAdmission=new HttpAdmission();
