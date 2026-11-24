#!/usr/bin/env node
// Two real HTTP origins for site-origin regressions.
//
// Page origin: serves a complete staged site with the Cloudflare Pages routing
// rules that matter here (documented in
// https://developers.cloudflare.com/pages/configuration/serving-pages/):
// `_redirects` root rule, `/x.html` -> `/x` and `/d/index.html` -> `/d/` 308
// redirects, extensionless lookup, the nearest `404.html` with status 404, and
// the single-page fallback (root index.html with status 200) when the stage has
// no top-level 404.html.
//
// Data origin: a reverse proxy of the real data origin (default
// https://gigapixels.imazen.org), forwarding Range/If-Range and returning the
// upstream status, body and range headers, with the bucket's CORS policy.
// `--data-mode proxy` forwards every path, including the old page copies that
// exist on the data origin today. `--data-mode redirect` forwards only data
// paths (/d/, /p/, /benchmarks/) and answers every other path with a 301 to the
// same section on the page origin (`/`, `/index.html` and
// `/gigapixel/index.html` go to `/gigapixel/`), keeping the query: the proposed
// alternate-origin rule.
//
// usage: node serve-site-origins.mjs STAGE PAGE_PORT DATA_PORT [--data-mode proxy|redirect] [--upstream URL]
import { createServer } from 'node:http';
import { readFile, stat } from 'node:fs/promises';
import { extname, join, normalize, relative, isAbsolute, sep } from 'node:path';

const args = process.argv.slice(2);
const opt = (name, fallback) => { const i = args.indexOf(name); return i >= 0 ? args.splice(i, 2)[1] : fallback; };
const dataMode = opt('--data-mode', 'proxy');
const upstream = new URL(opt('--upstream', 'https://gigapixels.imazen.org'));
const [stage, pagePortArg, dataPortArg] = args;
if (!stage || !pagePortArg || !dataPortArg || !['proxy', 'redirect'].includes(dataMode)) {
  throw new Error('usage: serve-site-origins.mjs STAGE PAGE_PORT DATA_PORT [--data-mode proxy|redirect] [--upstream URL]');
}
const pagePort = Number(pagePortArg), dataPort = Number(dataPortArg);
const pageOrigin = `http://127.0.0.1:${pagePort}`;
const MIME = {
  '.html': 'text/html; charset=utf-8', '.js': 'application/javascript', '.mjs': 'application/javascript',
  '.css': 'text/css; charset=utf-8', '.json': 'application/json', '.wasm': 'application/wasm',
  '.png': 'image/png', '.jpg': 'image/jpeg', '.svg': 'image/svg+xml', '.txt': 'text/plain',
};

async function file(path) {
  const full = join(stage, normalize(path));
  const inside = relative(stage, full);
  if (inside === '..' || inside.startsWith(`..${sep}`) || isAbsolute(inside)) return null;
  try { return (await stat(full)).isFile() ? full : null; } catch { return null; }
}

let rootRedirects = [];
try {
  rootRedirects = (await readFile(join(stage, '_redirects'), 'utf8')).split('\n').map(l => l.trim().split(/\s+/)).filter(p => p.length >= 2);
} catch {}

async function send(res, full, status = 200) {
  const body = await readFile(full);
  res.writeHead(status, { 'Content-Type': MIME[extname(full)] || 'application/octet-stream', 'Content-Length': body.length, 'Cache-Control': 'no-cache', 'Access-Control-Allow-Origin': '*' });
  res.end(body);
}

const pages = createServer(async (req, res) => {
  try {
    const url = new URL(req.url, pageOrigin);
    const path = decodeURIComponent(url.pathname);
    for (const [from, to, code] of rootRedirects) {
      if (from === path) { res.writeHead(Number(code || 302), { Location: to + url.search }).end(); return; }
    }
    if (/\/index\.html$/.test(path) && await file(path)) { res.writeHead(308, { Location: path.slice(0, -'index.html'.length) + url.search }).end(); return; }
    if (/\.html$/.test(path) && await file(path)) { res.writeHead(308, { Location: path.slice(0, -'.html'.length) + url.search }).end(); return; }
    const hit = path.endsWith('/') ? await file(path + 'index.html') : (await file(path)) || (await file(path + '.html'));
    if (hit) return send(res, hit);
    if (!path.endsWith('/') && await file(path + '/index.html')) { res.writeHead(308, { Location: path + '/' + url.search }).end(); return; }
    // Not found: nearest 404.html, else the single-page fallback.
    if (await file('404.html')) {
      let dir = path.slice(0, path.lastIndexOf('/') + 1);
      for (;;) {
        const nf = await file(dir + '404.html');
        if (nf) return send(res, nf, 404);
        if (dir === '/') break;
        dir = dir.slice(0, dir.slice(0, -1).lastIndexOf('/') + 1);
      }
    }
    return send(res, await file('index.html'), 200);
  } catch (e) {
    res.writeHead(500).end(String(e));
  }
});

const CORS = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Expose-Headers': 'Accept-Ranges,Content-Range,Content-Length,ETag,Content-Encoding',
};
const dataPath = p => /^\/(d|p|benchmarks)\//.test(p);
const data = createServer(async (req, res) => {
  try {
    const url = new URL(req.url, `http://127.0.0.1:${dataPort}`);
    if (req.method === 'OPTIONS') {
      res.writeHead(204, { ...CORS, 'Access-Control-Allow-Methods': 'GET, HEAD', 'Access-Control-Allow-Headers': req.headers['access-control-request-headers'] || 'range', 'Access-Control-Max-Age': '86400' }).end();
      return;
    }
    if (dataMode === 'redirect' && !dataPath(url.pathname)) {
      let path = url.pathname;
      if (path === '/' || path === '/index.html' || path === '/gigapixel/index.html') path = '/gigapixel/';
      res.writeHead(301, { Location: pageOrigin + path + url.search, 'Cache-Control': 'no-cache' }).end();
      return;
    }
    const headers = { 'User-Agent': 'IMI-site-origin-test/1.0', 'Accept-Encoding': 'identity' };
    for (const h of ['range', 'if-range', 'if-none-match']) if (req.headers[h]) headers[h] = req.headers[h];
    const up = await fetch(new URL(url.pathname + url.search, upstream), { method: req.method === 'HEAD' ? 'HEAD' : 'GET', headers, redirect: 'manual' });
    const out = { ...CORS };
    for (const h of ['content-type', 'content-length', 'content-range', 'accept-ranges', 'etag', 'cache-control', 'location', 'last-modified']) {
      const v = up.headers.get(h);
      if (v) out[h] = v;
    }
    res.writeHead(up.status, out);
    if (req.method === 'HEAD' || !up.body) { res.end(); return; }
    for await (const chunk of up.body) if (!res.write(chunk)) await new Promise(r => res.once('drain', r));
    res.end();
  } catch (e) {
    if (!res.headersSent) res.writeHead(502, CORS);
    res.end(String(e));
  }
});

pages.listen(pagePort, '127.0.0.1');
data.listen(dataPort, '127.0.0.1');
console.log(`page origin ${pageOrigin} (stage ${stage}); data origin http://127.0.0.1:${dataPort} (${dataMode} -> ${upstream.origin})`);
