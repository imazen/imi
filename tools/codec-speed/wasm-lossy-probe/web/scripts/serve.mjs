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
import { readFile, stat } from 'node:fs/promises';
import { homedir } from 'node:os';
import { dirname, extname, isAbsolute, join, normalize, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const [, , profileArg, portArg] = process.argv;
const profile = profileArg || 'plain';
const WEB = dirname(dirname(fileURLToPath(import.meta.url)));
const FIXTURES = process.env.IMIJ_FIXTURES || join(homedir(), 'tmp', 'imij-web-fixtures');
const BENCH = process.env.IMIJ_BENCH_FIXTURES || join(homedir(), 'tmp', 'imij-web-bench');
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
    const body = await readFile(full);
    const headers = { 'Content-Type': MIME[extname(full)] || 'application/octet-stream', ...headersFor(profile) };
    res.writeHead(200, headers).end(body);
  } catch (err) {
    res.writeHead(500).end(String(err));
  }
});

const requested = portArg ? Number(portArg) : 0;
server.listen(requested || 0, '127.0.0.1', () => {
  console.log(`LISTENING ${server.address().port}`);
});
