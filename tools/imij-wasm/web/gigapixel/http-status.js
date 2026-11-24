// The full accounting stays in Technical details; the overlay names current work.
export function httpStatus(http,failed=false){
  if(failed)return 'Failed · Retry';
  const v=http?.view||{},b=http?.background||{},parts=[];
  if(v.inflight)parts.push(`${v.inflight} active`);
  if(v.queued)parts.push(`${v.queued} queued`);
  if(v.pendingRanges)parts.push(`${v.pendingRanges} ranges`);
  if(parts.length&&v.failed)parts.push(`${v.failed} failed attempt${v.failed===1?'':'s'}`);
  if(b.inflight)parts.push(`${b.inflight} prefetch`);
  else if(b.queued)parts.push(`${b.queued} prefetch queued`);
  return parts.join(' · ');
}
