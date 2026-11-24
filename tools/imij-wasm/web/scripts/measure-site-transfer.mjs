// Measure actual CDN response bodies, then verify their decoded bytes against a stage.
// Node fetch transparently decompresses; use https.get to retain wire body bytes.
import https from 'node:https';
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { brotliDecompressSync, gunzipSync } from 'node:zlib';
import { createHash } from 'node:crypto';

const [stage, origin, output] = process.argv.slice(2);
if (!output) throw Error('usage: measure-site-transfer STAGE HTTPS_ORIGIN OUTPUT.json');
const base = new URL(origin);
if (base.protocol !== 'https:') throw Error('HTTPS origin required');
const entry = readFileSync(resolve(stage, 'gigapixel/index.html'), 'utf8');
const bundle = entry.match(/bundles\/([0-9a-f]{16})\/gigapixel\/viewer\.js/)?.[1];
if (!bundle) throw Error('entry does not name an immutable bundle');
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const results = [];
for (const variant of ['simd', 'relaxed']) {
  for (const name of ['imij.js', 'imij_bg.wasm']) {
    const path = `bundles/${bundle}/dist/pkg-${variant}/${name}`;
    const url = new URL(path, base);
    const { body, headers } = await new Promise((accept, reject) => {
      const request = https.get(url, { headers: { 'Accept-Encoding': 'br' } }, response => {
        if (response.statusCode !== 200) {
          response.resume();
          reject(Error(`${url}: HTTP ${response.statusCode}`));
          return;
        }
        const chunks = [];
        response.on('data', chunk => chunks.push(chunk));
        response.on('error', reject);
        response.on('end', () => accept({ body: Buffer.concat(chunks), headers: response.headers }));
      });
      request.setTimeout(30000, () => request.destroy(Error('response timeout')));
      request.on('error', reject);
    });
    const encoding = headers['content-encoding'] || 'identity';
    let decoded;
    if (encoding === 'br') decoded = brotliDecompressSync(body);
    else if (encoding === 'gzip') decoded = gunzipSync(body);
    else if (encoding === 'identity') decoded = body;
    else throw Error(`unexpected content encoding ${encoding}`);
    const expected = readFileSync(resolve(stage, path));
    if (!decoded.equals(expected)) throw Error(`CDN bytes differ from stage: ${path}`);
    results.push({ variant, name, url: url.href, encoding,
      response_body_bytes: body.length, decoded_bytes: decoded.length,
      response_sha256: hash(body), decoded_sha256: hash(decoded),
      etag: headers.etag, cache_status: headers['cf-cache-status'] });
  }
}
const summary = { observed_utc: new Date().toISOString(), bundle,
  scope: 'HTTP response bodies only; excludes headers, transport overhead and other page assets.',
  results, combined_response_body_bytes: Object.fromEntries(['simd', 'relaxed'].map(variant =>
    [variant, results.filter(r => r.variant === variant).reduce((sum, r) => sum + r.response_body_bytes, 0)])) };
writeFileSync(output, JSON.stringify(summary, null, 2) + '\n');
console.log(JSON.stringify(summary));
