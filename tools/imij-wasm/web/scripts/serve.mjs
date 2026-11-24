#!/usr/bin/env node
// Static file server for the imij browser tests, in three header profiles
// (all serve the same tree):
//   plain    - no COOP/COEP, no CSP: `pkg-threads` cannot be used (the page is
//              not cross-origin isolated), so the pool falls back to `pkg-simd`.
//   isolated - `Cross-Origin-Opener-Policy: same-origin` + `Cross-Origin-Embedder-Policy:
//              require-corp`: the page is cross-origin isolated, SharedArrayBuffer works,
//              `pkg-threads` runs and its rayon pool spreads one decode over Web Workers.
//   strict   - isolated headers plus a CSP restrictive enough to prove the wasm
//              packages need nothing beyond it: `default-src 'self'; script-src 'self'
//              'wasm-unsafe-eval'; worker-src 'self' blob:; img-src 'self'; style-src
//              'self' 'unsafe-inline'; connect-src 'self'; base-uri 'none'`. `worker-src`
//              needs `blob:` for wasm-bindgen-rayon's bundler-less child-worker spawn
//              (workerHelpers.no-bundler.js fetches its own script as a blob and starts a
//              Worker from the object URL); without it initThreadPool hangs forever on
//              child workers a bare CSP silently blocked (zenjpegai, 2026-09-17).
//
// The served root is this directory (tools/imij-wasm/web); `/fixtures/*` maps to
// $IMIJ_FIXTURES (default $HOME/tmp/imij-web-fixtures) and `/bench/*` to
// $IMIJ_BENCH_FIXTURES — the frozen parity sets and the generated timing streams.
//
// usage: node scripts/serve.mjs <profile> <port>
import { createServer } from 'node:http';
import { createReadStream } from 'node:fs';
import { readFile, stat } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, extname, isAbsolute, join, normalize, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const [, , profileArg, portArg] = process.argv;
const profile = profileArg || 'plain';
const WEB = process.env.IMIJ_SITE_STAGE || dirname(dirname(fileURLToPath(import.meta.url)));
const FIXTURES = process.env.IMIJ_FIXTURES || join(homedir(), 'tmp', 'imij-web-fixtures');
const BENCH = process.env.IMIJ_BENCH_FIXTURES || join(homedir(), 'tmp', 'imij-web-bench');
const DIST = process.env.IMIJ_WASM_DIST || join(WEB, 'dist');
const ATLAS = process.env.IMIJ_ATLAS_STAGE;
const V4_VIEWER = process.env.IMIJ_V4_VIEWER;
const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.wasm': 'application/wasm',
  '.json': 'application/json',
  '.tsv': 'text/tab-separated-values; charset=utf-8',
  '.png': 'image/png',
  '.css': 'text/css; charset=utf-8',
  '.imi': 'application/octet-stream',
  '.raw': 'application/octet-stream',
};

function headersFor(profile) {
  const h = {};
  if (profile === 'isolated' || profile === 'strict') {
    h['Cross-Origin-Opener-Policy'] = 'same-origin';
    h['Cross-Origin-Embedder-Policy'] = 'require-corp';
  }
  if (profile === 'strict') {
    h['Content-Security-Policy'] =
      "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self' blob:; " +
      "img-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; base-uri 'none'";
  }
  return h;
}

const server = createServer(async (req, res) => {
  try {
    const url = new URL(req.url, 'http://localhost');
    let path = decodeURIComponent(url.pathname);
    if (path.endsWith('/')) path += 'index.html';
    // Fixture and bench trees live in scratch, outside this checkout.
    const [root, rel] = path.startsWith('/fixtures/')
      ? [FIXTURES, path.slice('/fixtures/'.length)]
      : path.startsWith('/bench/')
        ? [BENCH, path.slice('/bench/'.length)]
        : path.startsWith('/dist/')
          ? [DIST, path.slice('/dist/'.length)]
          : path.startsWith('/image/') && V4_VIEWER
            ? [V4_VIEWER, path.slice('/image/'.length)]
          : path.startsWith('/atlas/') && ATLAS
            ? [ATLAS, path.slice('/atlas/'.length)]
            : [WEB, path.slice(1)];
    const full = join(root, normalize(rel));
    // Containment by path components: a string prefix test would admit a
    // sibling directory whose name starts with the root's name.
    const inside = relative(root, full);
    if (inside === '..' || inside.startsWith(`..${sep}`) || isAbsolute(inside)) {
      res.writeHead(403).end('forbidden');
      return;
    }
    const st = await stat(full).catch(() => null);
    if (!st || !st.isFile()) {
      res.writeHead(404).end('not found');
      return;
    }
    const archive = ['.imijset', '.imiset'].includes(extname(full));
    const body = archive || req.method === 'HEAD' ? null : await readFile(full);
    const send = (start, end) => {
      if (body) return res.end(body.subarray(start, end + 1));
      if (end < start) return res.end();
      const stream = createReadStream(full, {start, end});
      stream.on('error', error => res.destroy(error));
      res.once('close', () => stream.destroy());
      stream.pipe(res);
    };
    const headers = { 'Content-Type': MIME[extname(full)] || 'application/octet-stream', ...headersFor(profile) };
    // Current V4 integration exercises the real range transport against files,
    // including held response bodies. Existing plain fixture requests stay 200.
    headers['Accept-Ranges'] = 'bytes';
    headers.ETag = `"${st.size.toString(16)}-${st.mtimeMs.toString(16)}"`;
    if (req.method === 'HEAD') { res.writeHead(200, {...headers, 'Content-Length': st.size}).end(); return; }
    if (req.headers['if-range'] && req.headers['if-range'] !== headers.ETag) { res.writeHead(412, headers).end(); return; }
    if (req.headers.range) {
      const match = /^bytes=(\d+)-(\d+)$/.exec(req.headers.range);
      const start = Number(match?.[1]), end = Number(match?.[2]);
      if (!match || !Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start > end || end >= st.size) {
        res.writeHead(416, {...headers, 'Content-Range': `bytes */${st.size}`}).end(); return;
      }
      res.writeHead(206, {...headers, 'Content-Length': end-start+1, 'Content-Range': `bytes ${start}-${end}/${st.size}`});
      const delay = process.env.IMIJ_V4 ? Number(url.searchParams.get('v4-delay')) : 0;
      if (delay > 0 && delay <= 1000) { res.flushHeaders(); const timer=setTimeout(()=>send(start,end),delay); res.on('close',()=>clearTimeout(timer)); }
      else send(start,end);
      return;
    }
    res.writeHead(200, {...headers, 'Content-Length': st.size});
    send(0, st.size - 1);
  } catch (err) {
    res.writeHead(500).end(String(err));
  }
});

const requested = portArg ? Number(portArg) : 0;
server.listen(requested || 0, process.env.IMIJ_WEB_BIND || '127.0.0.1', () => {
  console.log(`LISTENING ${server.address().port}`);
});
