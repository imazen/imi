// Serve an R2 bucket at a local origin (same-origin testing before a custom domain exists): objects are read with
// signed S3 GET/HEAD requests (Range, ETag and status pass through unchanged). Paths under /d/ and /p/ always come
// from the bucket; any other path comes from STATIC_DIR when given, else from the bucket. Read-only; credentials come
// from R2_ACCOUNT_ID / R2_ACCESS_KEY_ID / R2_SECRET_ACCESS_KEY and are never logged.
// usage: serve-r2-bucket.mjs BUCKET PORT [STATIC_DIR]
import {createHash,createHmac} from 'node:crypto';
import {createServer} from 'node:http';
import {readFileSync,statSync} from 'node:fs';
import {extname,join,normalize} from 'node:path';
import {Readable} from 'node:stream';
const [bucket,port,staticDir]=process.argv.slice(2);
if(!port)throw Error('usage: serve-r2-bucket BUCKET PORT [STATIC_DIR]');
const {R2_ACCOUNT_ID:account,R2_ACCESS_KEY_ID:keyId,R2_SECRET_ACCESS_KEY:secret}=process.env;
if(!account||!keyId||!secret)throw Error('R2 credentials missing from the environment');
const host=`${account}.r2.cloudflarestorage.com`,hmac=(key,data)=>createHmac('sha256',key).update(data).digest();
const encode=segment=>encodeURIComponent(segment).replace(/[!'()*]/g,c=>'%'+c.charCodeAt(0).toString(16).toUpperCase());
function sign(method,key,range){
  const now=new Date().toISOString().replace(/[:-]|\.\d{3}/g,''),day=now.slice(0,8),uri=`/${bucket}/${key.split('/').map(encode).join('/')}`;
  const headers={host,'x-amz-content-sha256':'UNSIGNED-PAYLOAD','x-amz-date':now,...(range?{range}:{})};
  const names=Object.keys(headers).sort(),canonical=`${method}\n${uri}\n\n${names.map(n=>`${n}:${headers[n]}\n`).join('')}\n${names.join(';')}\nUNSIGNED-PAYLOAD`;
  const scope=`${day}/auto/s3/aws4_request`,toSign=`AWS4-HMAC-SHA256\n${now}\n${scope}\n${createHash('sha256').update(canonical).digest('hex')}`;
  const signature=createHmac('sha256',hmac(hmac(hmac(hmac(`AWS4${secret}`,day),'auto'),'s3'),'aws4_request')).update(toSign).digest('hex');
  return {url:`https://${host}${uri}`,headers:{...headers,authorization:`AWS4-HMAC-SHA256 Credential=${keyId}/${scope}, SignedHeaders=${names.join(';')}, Signature=${signature}`}};
}
const types={'.html':'text/html','.js':'text/javascript','.json':'application/json','.css':'text/css','.wasm':'application/wasm','.jpg':'image/jpeg','.bin':'application/octet-stream'};
createServer(async(req,res)=>{
  try{
    const url=new URL(req.url,'http://x'),path=decodeURIComponent(url.pathname);
    if(!['GET','HEAD'].includes(req.method)){res.writeHead(405);return res.end();}
    if(staticDir&&!path.startsWith('/d/')&&!path.startsWith('/p/')){
      let file=join(staticDir,normalize(path).replace(/^(\.\.[/\\])+/,''));
      try{if(statSync(file).isDirectory())file=join(file,'index.html');const body=readFileSync(file);res.writeHead(200,{'content-type':types[extname(file)]||'application/octet-stream','cache-control':'no-store'});return res.end(req.method==='HEAD'?undefined:body);}
      catch{res.writeHead(404);return res.end();}
    }
    const key=path.replace(/^\//,'')||'index.html',{url:target,headers}=sign(req.method,key,req.headers.range);
    const response=await fetch(target,{method:req.method,headers});
    const out={};for(const name of ['content-type','content-length','content-range','accept-ranges','etag','cache-control','last-modified'])if(response.headers.get(name))out[name]=response.headers.get(name);
    res.writeHead(response.status,out);
    if(req.method==='HEAD'||!response.body)return res.end();
    Readable.fromWeb(response.body).on('error',()=>res.destroy()).pipe(res);
  }catch(error){console.error('proxy error',String(error).slice(0,160));if(!res.headersSent)res.writeHead(502);res.end();}
}).listen(Number(port),'127.0.0.1',()=>console.log(`serving ${bucket} on http://127.0.0.1:${port}/`));
