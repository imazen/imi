// A static/range server: it never decodes or resizes images.
import { createServer } from 'node:http';
import { createReadStream } from 'node:fs';
import { stat, readFile } from 'node:fs/promises';
import { dirname, extname, resolve, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { once } from 'node:events';

const web = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const data = resolve(process.argv[2] || '.');
const port = Number(process.argv[3] || 3321);
if (!Number.isInteger(port) || port < 3000 || port > 3999) throw Error('port must be 3000..3999');
const mime = { '.html':'text/html; charset=utf-8', '.js':'text/javascript', '.mjs':'text/javascript',
  '.json':'application/json', '.css':'text/css', '.wasm':'application/wasm', '.png':'image/png' };

export function parseRanges(value, size) {
  if (!value?.startsWith('bytes=')) throw Error('invalid Range');
  const parts = value.slice(6).split(',');
  if (!parts.length || parts.length > 256) throw Error('range count');
  return parts.map(part => {
    const match = /^(\d+)-(\d+)$/.exec(part.trim());
    if (!match) throw Error('explicit bounded byte ranges required');
    const start = Number(match[1]), end = Number(match[2]);
    if (![start,end].every(Number.isSafeInteger) || start < 0 || end < start || end >= size) throw Error('range extent');
    return { start, end };
  });
}

const server = createServer({ maxHeaderSize: 32768 }, async (req, res) => {
  try {
    if (!['GET','HEAD'].includes(req.method)) { res.writeHead(405).end(); return; }
    const path = decodeURIComponent(new URL(req.url, 'http://server').pathname);
    if(path==='/'){res.writeHead(302,{Location:'/gigapixel/'}).end();return;}
    const root = path.startsWith('/image/') ? data : web;
    const rel = path.startsWith('/image/') ? path.slice(7) : path === '/gigapixel/' ? 'gigapixel/index.html' : path.slice(1);
    const file = resolve(root, rel);
    const inside = relative(root, file);
    if (inside.startsWith(`..${sep}`) || inside === '..') { res.writeHead(403).end(); return; }
    // Only named demo data is served: original PNG/raw source rasters stay private.
    if (root === data && !['image.json','blue-marble.imi'].includes(rel)) { res.writeHead(404).end(); return; }
    const s = await stat(file);
    if (!s.isFile()) { res.writeHead(404).end(); return; }
    const etag = `"${s.size.toString(16)}-${s.mtimeNs?.toString(16) || s.mtimeMs.toString(16)}"`;
    const headers = { 'Content-Type':mime[extname(file)] || 'application/octet-stream',
      'Accept-Ranges':'bytes', 'ETag':etag, 'Cache-Control':'no-cache',
      ...(process.env.IMIJ_DEMO_ISOLATED==='0'?{}:{'Cross-Origin-Opener-Policy':'same-origin', 'Cross-Origin-Embedder-Policy':'require-corp'}) };
    if(root===web && extname(file)==='.wasm' && !req.headers.range){
      const accepted=(req.headers['accept-encoding']||'').split(',').map(v=>v.trim().split(';')[0]);
      const encoding=accepted.includes('br')?'br':accepted.includes('gzip')?'gzip':null;
      if(encoding){
        const compressed=file+(encoding==='br'?'.br':'.gz');
        const packed=await stat(compressed).catch(()=>null);
        if(packed?.isFile()){
          res.writeHead(200,{...headers,'Content-Encoding':encoding,'Vary':'Accept-Encoding','Content-Length':packed.size});
          if(req.method==='HEAD')res.end();else createReadStream(compressed).on('error',e=>res.destroy(e)).pipe(res);
          return;
        }
      }
    }
    if (req.method === 'HEAD') { res.writeHead(200, { ...headers, 'Content-Length':s.size }).end(); return; }
    if (req.headers['if-range'] && req.headers['if-range'] !== etag) { res.writeHead(412, headers).end('image identity changed'); return; }
    if (req.headers.range) {
      let ranges;
      try { ranges = parseRanges(req.headers.range, s.size); }
      catch { res.writeHead(416, { ...headers, 'Content-Range':`bytes */${s.size}` }).end(); return; }
      if (ranges.reduce((n,r)=>n+r.end-r.start+1,0) > 32*1024*1024) { res.writeHead(416, headers).end('range response budget'); return; }
      if (ranges.length === 1) {
        const r = ranges[0];
        res.writeHead(206, { ...headers, 'Content-Length':r.end-r.start+1, 'Content-Range':`bytes ${r.start}-${r.end}/${s.size}` });
        const stream = createReadStream(file, r);
        stream.on('error', e=>res.destroy(e));
        res.on('close', ()=>stream.destroy()); stream.pipe(res); return;
      }
      const boundary = 'imij-range-boundary';
      res.writeHead(206, { ...headers, 'Content-Type':`multipart/byteranges; boundary=${boundary}` });
      for (const r of ranges) {
        if (res.destroyed) return;
        res.write(`--${boundary}\r\nContent-Type: application/octet-stream\r\nContent-Range: bytes ${r.start}-${r.end}/${s.size}\r\n\r\n`);
        const stream = createReadStream(file,r);
        const close = ()=>stream.destroy(); res.once('close',close);
        try { for await (const bytes of stream) { if (!res.write(bytes)) await once(res,'drain'); } }
        finally { res.off('close',close); stream.destroy(); }
        res.write('\r\n');
      }
      res.end(`--${boundary}--\r\n`); return;
    }
    if (root === data && rel.endsWith('.imi')) { res.writeHead(400,headers).end('Range required for image'); return; }
    res.writeHead(200, { ...headers, 'Content-Length':s.size });
    res.end(await readFile(file));
  } catch (e) { if (!res.headersSent) res.writeHead(e.code === 'ENOENT' ? 404 : 500); res.end(String(e)); }
});
server.listen(port, '0.0.0.0', ()=>console.log(`LISTENING ${port}`));
