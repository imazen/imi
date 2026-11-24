// Read-only directory/cache qualification against the actual public CDN.
// Network observations are not a cold-edge guarantee or an isolated CPU benchmark.
import {mkdirSync, readFileSync, writeFileSync} from 'node:fs';
import {resolve} from 'node:path';
import {pathToFileURL} from 'node:url';

const [web, output, configuration] = process.argv.slice(2);
if (!output) throw Error('WEB FRESH_OUTPUT [CONFIG.json]');
mkdirSync(output, {recursive: false});
const config = configuration ? JSON.parse(readFileSync(configuration, 'utf8')) : {};
writeFileSync(resolve(output, 'CONFIG.json'), JSON.stringify(config, null, 2));
const {chromium} = await import(pathToFileURL(resolve(web, 'node_modules/playwright-core/index.mjs')).href);
const browser = await chromium.launch({channel: 'chromium', args: config.browserArgs ?? []});
const results = [];
const origin = 'https://imi.imazen.org';
const catalogUrl = `${origin}/image/image.json`;
const indexCode = readFileSync(resolve(web, 'gigapixel/tile-index.js'), 'utf8')
  .replace('export class TileIndex', 'globalThis.ProbeTileIndex = class TileIndex');
async function context() {
  const ctx = await browser.newContext();
  const page = await ctx.newPage();
  const session = await ctx.newCDPSession(page);
  await session.send('Network.enable');
  const events = [], requestUrls = new Map(), requestMethods = new Map(), responses = new Map();
  const cacheRequests = new Set(), finished = new Map(), wireStatus = new Map();
  session.on('Network.requestWillBeSent', ({requestId, request}) => {
    requestUrls.set(requestId, request.url); requestMethods.set(requestId, request.method);
  });
  session.on('Network.requestServedFromCache', ({requestId}) => {
    cacheRequests.add(requestId);
    if (responses.has(requestId)) responses.get(requestId).servedFromCache = true;
  });
  session.on('Network.loadingFinished', ({requestId, encodedDataLength}) => {
    finished.set(requestId, encodedDataLength);
    if (responses.has(requestId)) responses.get(requestId).encodedDataLength = encodedDataLength;
  });
  session.on('Network.responseReceivedExtraInfo', ({requestId, statusCode}) => {
    wireStatus.set(requestId, statusCode);
    if (responses.has(requestId)) responses.get(requestId).wireStatus = statusCode;
  });
  session.on('Network.loadingFailed', ({requestId, errorText, canceled}) => events.push({
    requestId, url: requestUrls.get(requestId), method: requestMethods.get(requestId), errorText, canceled, failed: true,
  }));
  session.on('Network.responseReceived', ({requestId, response}) => {
    const record = {requestId, url: response.url, method: requestMethods.get(requestId), status: response.status, protocol: response.protocol,
      headers: response.headers, fromDiskCache: !!response.fromDiskCache,
      servedFromCache: cacheRequests.has(requestId), encodedDataLength: finished.get(requestId),
      wireStatus: wireStatus.get(requestId), fromPrefetchCache: !!response.fromPrefetchCache,
      fromServiceWorker: !!response.fromServiceWorker, timing: response.timing,
      connectionReused: response.connectionReused};
    responses.set(requestId, record); events.push(record);
  });
  await page.goto(`${origin}/imi/validation`, {waitUntil: 'domcontentloaded'});
  await page.addScriptTag({content: indexCode});
  return {ctx, page, events};
}
try {
  const discovery = await context();
  const catalog = await discovery.page.evaluate(async url => {
    const r = await fetch(url); if (!r.ok) throw Error(`catalog ${r.status}`);
    return await r.json();
  }, catalogUrl);
  writeFileSync(resolve(output, 'catalog.json'), JSON.stringify(catalog, null, 2));
  writeFileSync(resolve(output, 'discovery-network.json'), JSON.stringify(discovery.events, null, 2));
  console.log(JSON.stringify({stage: 'catalog', keys: Object.keys(catalog)}));
  await discovery.ctx.close();
  for (let trial = 0; trial < (config.trials ?? 3); trial++) {
    const jobs = trial % 2 ? [...(config.jobs ?? [])].reverse() : config.jobs ?? [];
    for (const job of jobs) {
      const {ctx, page, events} = await context();
      try {
        for (let repeat = 0; repeat < (config.repeats ?? 2); repeat++) {
          const before = events.length;
          let value;
          try { value = await page.evaluate(async job => {
            if (job.viewerSource) {
              const spec = job.viewerSource;
              const {HttpRangeSource} = await import(spec.module);
              const retained = globalThis.viewerProbeSources ??= new Map();
              let source = spec.retain ? retained.get(job.label) : null;
              const before = source?.stats() ?? {requests: 0, headRequests: 0, bytes: 0};
              const started = performance.now();
              if (!source) {
                // Match worker.js: browser cache enabled, eight application slots,
                // per-URL cache-write serialization, multipart off by default.
                const capacity = ((navigator.deviceMemory || 8) >= 8 ? 192 : 96) * 1024 ** 2;
                const options = {cacheMode: 'default', parallel: 8, capacity,
                  gap: 4096, multipart: false};
                source = spec.parts ? HttpRangeSource.openSegmented(spec.parts.base, spec.length, spec.parts, () => {}, options)
                  : spec.knownLength ? new HttpRangeSource(spec.url, spec.length, spec.etag, () => {}, options)
                    : await HttpRangeSource.open(spec.url, () => {}, options);
                source.opening = false;
                if (spec.retain) retained.set(job.label, source);
              }
              await source.ensureSpans(job.spans);
              const fetchMs = performance.now() - started;
              const bodies = job.spans.map(({start, end}) => {
                const bytes = source.read(start, end - start, false);
                if (!bytes) throw Error('viewer source did not supply requested span');
                return bytes;
              });
              const after = source.stats();
              const bodiesBase64 = job.saveBody ? bodies.map(bytes => {
                let binary = '';
                for (let at = 0; at < bytes.length; at += 16384)
                  binary += String.fromCharCode(...bytes.subarray(at, at + 16384));
                return btoa(binary);
              }) : undefined;
              return {bytes: bodies.reduce((n, b) => n + b.length, 0), fetchMs, bodiesBase64,
                sourceRequests: after.requests - before.requests,
                sourceHeadRequests: after.headRequests - before.headRequests,
                sourceFetchedBytes: after.bytes - before.bytes, sourceCapacity: source.capacity, sourceStats: after};
            }
            if (job.requests) {
              // Stages explicitly distinguish known parallel ranges from dependent
              // reads. Directory-page controls do not implement a paged codec.
              const started = performance.now(), rows = [], bodies = [];
              const stages = [...new Set(job.requests.map(r => r.stage ?? 0))].sort((a, b) => a - b);
              for (const stage of stages) {
                await Promise.all(job.requests.map(async (request, i) => {
                  if ((request.stage ?? 0) !== stage) return;
                  const start = performance.now();
                  const response = await fetch(request.url, {headers: {Range: request.range}, signal: AbortSignal.timeout(30000)});
                  const headersAt = performance.now(), bytes = new Uint8Array(await response.arrayBuffer());
                  const end = performance.now(), extent = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(response.headers.get('content-range') ?? '');
                  if (response.status !== 206 || !extent || +extent[2] - +extent[1] + 1 !== bytes.length ||
                    request.total !== undefined && +extent[3] !== request.total)
                    throw Error(`${job.label} range framing ${i}`);
                  const expected = /^bytes=(\d+)-(\d+)$/.exec(request.range);
                  if (expected && (+extent[1] !== +expected[1] || +extent[2] !== +expected[2]))
                    throw Error(`${job.label} range mismatch ${i}`);
                  rows[i] = {stage, startMs: start - started, headersMs: headersAt - start,
                    endMs: end - started, bytes: bytes.length, contentRange: response.headers.get('content-range')};
                  if (job.saveBody) bodies[i] = bytes;
                }));
              }
              const fetchMs = Math.max(...rows.map(r => r.endMs));
              const bodiesBase64 = job.saveBody ? bodies.map(bytes => {
                let binary = '';
                for (let at = 0; at < bytes.length; at += 16384)
                  binary += String.fromCharCode(...bytes.subarray(at, at + 16384));
                return btoa(binary);
              }) : undefined;
              return {bytes: rows.reduce((n, r) => n + r.bytes, 0), fetchMs, requests: rows, bodiesBase64};
            }
            const headers = job.range ? {Range: job.range} : {};
            const started = performance.now(), response = await fetch(job.url, {headers, signal: AbortSignal.timeout(30000)});
            const headersAt = performance.now(), bytes = new Uint8Array(await response.arrayBuffer());
            const bodyAt = performance.now();
            if (job.range ? response.status !== 206 : response.status !== 200)
              throw Error(`${job.label} status ${response.status}`);
            let legacyHashMs = null;
            if (job.expectedLegacySha256) {
              // This is the existing catalog's protocol oracle, not a new identity scheme.
              const started = performance.now();
              const digest = new Uint8Array(await crypto.subtle.digest('SHA-256', bytes));
              if (Array.from(digest, x => x.toString(16).padStart(2, '0')).join('') !== job.expectedLegacySha256)
                throw Error('catalog sidecar identity');
              legacyHashMs = performance.now() - started;
            }
            let parsed = null;
            if (job.index) {
              const started = performance.now();
              const index = new globalThis.ProbeTileIndex(bytes, job.index, job.index.length);
              parsed = {entries: index.length, parseMs: performance.now() - started};
            }
            let bodyBase64;
            if (job.saveBody) {
              let binary = '';
              for (let at = 0; at < bytes.length; at += 16384)
                binary += String.fromCharCode(...bytes.subarray(at, at + 16384));
              bodyBase64 = btoa(binary);
            }
            return {status: response.status, bytes: bytes.length,
              headersMs: headersAt - started, bodyMs: bodyAt - headersAt,
              fetchMs: bodyAt - started, legacyHashMs, parsed, bodyBase64};
          }, {...job, saveBody: job.saveBody && (config.retainAllBodies || trial === 0 && repeat === 0)});
          } catch (error) {
            results.push({label: job.label, trial, repeat, error: String(error), network: events.slice(before)});
            writeFileSync(resolve(output, 'partial.json'), JSON.stringify(results, null, 2));
            throw error;
          }
          if (value.bodyBase64) {
            writeFileSync(resolve(output, `${job.label}.bin`), Buffer.from(value.bodyBase64, 'base64'));
            delete value.bodyBase64;
          }
          if (value.bodiesBase64) {
            const name = `${job.label}-${trial}-${repeat}.bin`;
            writeFileSync(resolve(output, name), Buffer.concat(value.bodiesBase64.map(b => Buffer.from(b, 'base64'))));
            value.bodyFile = name;
            delete value.bodiesBase64;
          }
          const urls = new Set(job.requests?.map(r => r.url) ?? [job.url]);
          const network = events.slice(before).filter(x => job.viewerSource
            ? job.viewerSource.parts ? x.url?.startsWith(job.viewerSource.parts.base + '/') : x.url === job.viewerSource.url
            : urls.has(x.url));
          const row = {label: job.label, trial, repeat, ...value, network};
          results.push(row);
          writeFileSync(resolve(output, 'partial.json'), JSON.stringify(results, null, 2));
          console.log(JSON.stringify({...row, network: network.map(x => ({status: x.status, protocol: x.protocol,
            fromDiskCache: x.fromDiskCache, cache: x.headers?.['cf-cache-status'] ?? x.headers?.['CF-Cache-Status']}))}));
        }
      } finally { await ctx.close(); }
    }
  }
  writeFileSync(resolve(output, 'results.json'), JSON.stringify({version: browser.version(), config, results}, null, 2));
} finally { await browser.close(); }
