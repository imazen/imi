// Incremental union of completed worker spans. Concurrent workers count once;
// HTTP gaps never enter the denominator. Inserting each completion avoids
// sorting the entire growing view history for every tile/progress message.
export class DecodeMeter {
  constructor(){this.intervals=[];this.ms=0;this.pixels=0;}
  addSpans(spans){
    for(const [start,stop] of spans){
      if(!(stop>start))continue;
      let lo=0,hi=this.intervals.length;
      while(lo<hi){const mid=(lo+hi)>>>1;if(this.intervals[mid][1]<start)lo=mid+1;else hi=mid;}
      let end=lo,a=start,b=stop;
      while(end<this.intervals.length&&this.intervals[end][0]<=b){
        const old=this.intervals[end++];a=Math.min(a,old[0]);b=Math.max(b,old[1]);this.ms-=old[1]-old[0];
      }
      this.intervals.splice(lo,end-lo,[a,b]);this.ms+=b-a;
    }
  }
  snapshot(requestId,workers){return this.pixels>0&&this.ms>0?{requestId,mpxPerSecond:this.pixels/(this.ms*1000),pixels:this.pixels,ms:this.ms,workers}:null;}
}
