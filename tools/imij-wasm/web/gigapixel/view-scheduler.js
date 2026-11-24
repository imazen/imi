// Send the newest viewport on the next animation frame, even while an
// earlier view is decoding. The worker owns supersession and HTTP admission.
export class ViewScheduler{
  constructor(dispatch,blocked,now,raf,clearFrame){
    this.dispatch=dispatch;this.blocked=blocked;this.now=now;this.raf=raf;this.clearFrame=clearFrame;
    this.pending=null;this.pendingAt=0;this.dispatchAt=0;
  }
  request(){
    if(this.blocked())return;
    this.pendingAt=this.now();
    if(this.pending===null)this.pending=this.raf(this.fire);
  }
  fire=()=>{
    this.pending=null;
    if(this.blocked())return;
    this.dispatchAt=this.now();this.dispatch(this.pendingAt);
  };
  cancel(){
    if(this.pending!==null)this.clearFrame(this.pending);
    this.pending=null;
  }
}
