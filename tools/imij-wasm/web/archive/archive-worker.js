import init, {RangeArchive, decodeRgba} from '../dist/pkg-simd/imij.js';
import {HttpRangeSource} from '../gigapixel/range-source.js';
import {mergeAdjacent} from '../gigapixel/prefetch.js';
import {ARCHIVE_CACHE_BYTES, rangeDemand} from './range-demand.js';
import {PROFILE, CATALOG_BYTES, PAGE_BYTES, ITEM_BYTES, THUMBNAIL_BYTES,
        attribute, parseCatalog, parsePage, pageKey, key} from './catalog.js';

const ready = init();
const options = {cacheMode: 'default', multipart: false, capacity: ARCHIVE_CACHE_BYTES};
const previews = new Set(['image/png', 'image/jpeg', 'image/webp', 'image/gif', 'image/bmp', 'image/avif']);
const imijMimes = new Set(['image/imi', 'image/imij', 'image/x-imi', 'application/vnd.imij']);
let session;

async function openSource(input) {
  if (input instanceof File) {
    const source = new HttpRangeSource('', input.size, null, () => {}, options);
    return {source, ensure: async spans => {
      for (const {start, end} of spans) {
        source.add(start, new Uint8Array(await input.slice(start, end).arrayBuffer()));
      }
      source.trim();
    }};
  }
  const url = new URL(input);
  if (!['http:', 'https:'].includes(url.protocol)) throw Error('Use an HTTP or HTTPS archive URL.');
  const source = await HttpRangeSource.open(url, () => {}, options);
  return {source, ensure: spans => source.ensureSpans(spans)};
}

async function attach(input) {
  const {source, ensure} = await openSource(input);
  const {read, readMany, demand} = rangeDemand(source, ensure);
  const reader = await demand(() => new RangeArchive(source.length, read, readMany));
  source.opening = false;
  const entries = keys => demand(() => reader.entries(BigUint64Array.from(keys, key)));
  const load = (entryKey, max) => demand(() => reader.readEntry(key(entryKey), max));
  try {
    const [root] = await entries(['0']);
    const catalog = attribute(root, 'imij.profile') === PROFILE
      ? parseCatalog(await load('0', CATALOG_BYTES)) : null;
    return {reader, source, ensure, demand, load, entries, catalog};
  } catch (error) {
    reader.free();
    throw error;
  }
}

function mime(entry) {
  const explicit = attribute(entry, 'media_type');
  if (explicit) return explicit;
  const extension = (attribute(entry, 'format') || entry?.name?.split('.').pop() || '').toLowerCase();
  return ({png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', webp: 'image/webp',
           gif: 'image/gif', bmp: 'image/bmp', avif: 'image/avif', json: 'application/json',
           tsv: 'text/tab-separated-values'})[extension];
}

function imijPreview(entry, media) {
  if (!entry.width || !entry.height) return false;
  const name = attribute(entry, 'format') || entry.name || '';
  return !media || imijMimes.has(media) ||
    (media === 'application/octet-stream' && /(?:^|\.)imij?$/i.test(name));
}

function requireEntry(entry, asset) {
  if (!entry) throw Error(`Archive entry ${asset.key} is missing.`);
  if (asset.digest && typeof asset.digest === 'object' &&
      (entry.digestId !== asset.digest.id || entry.digestBytes !== asset.digest.bytes)) {
    throw Error('Scoring input differs from the catalog binding.');
  }
}

async function fetchAsset(asset) {
  if (asset.ordinal !== undefined) {
    return session.demand(() => session.reader.decodeItem(key(asset.key), key(asset.ordinal)));
  }
  return {bytes: await session.load(asset.key, ITEM_BYTES)};
}

async function prefetchEntries(entries, maxEach) {
  const available = entries.filter(entry => entry.bytes > 0 && entry.bytes <= maxEach);
  if (available.reduce((sum, entry) => sum + entry.bytes, 0) <= ITEM_BYTES) {
    await session.ensure(mergeAdjacent(available.map(entry => ({
      start: entry.offset, end: entry.offset + entry.bytes,
    }))));
  }
}

const handlers = {
  async open(input) {
    await ready;
    session?.reader.free();
    session = null;
    session = await attach(input);
    return {snapshot: session.reader.snapshot(), catalog: session.catalog};
  },
  async list(start, count) {
    return session.demand(() => session.reader.list(key(start), count));
  },
  async pairs(index) {
    const entryKey = pageKey(session.catalog, index);
    return parsePage(await session.load(entryKey.toString(), PAGE_BYTES), session.catalog, index);
  },
  async previews(keys, thumbnails) {
    const descriptions = await session.entries(keys);
    if (thumbnails) await prefetchEntries(descriptions.filter(entry => entry && previews.has(mime(entry))), THUMBNAIL_BYTES);
    const output = [];
    for (const [i, entry] of descriptions.entries()) {
      if (!entry) throw Error(`Archive entry ${keys[i]} is missing.`);
      const media = mime(entry);
      if (previews.has(media)) {
        const bytes = await session.load(entry.key, thumbnails ? THUMBNAIL_BYTES : ITEM_BYTES);
        output.push({bytes, media});
      } else if (!thumbnails && ['application/json', 'text/tab-separated-values'].includes(media)) {
        const text = new TextDecoder('utf-8', {fatal: true}).decode(await session.load(entry.key, PAGE_BYTES));
        output.push({text, media});
      } else if (!thumbnails && imijPreview(entry, media)) {
        const image = decodeRgba(await session.load(entry.key, ITEM_BYTES), 1);
        output.push({rgba: image.rgba, width: image.width, height: image.height});
      } else {
        output.push(null);
      }
    }
    return output;
  },
  async exact(asset) {
    const [entry] = await session.entries([asset.key]);
    requireEntry(entry, asset);
    return fetchAsset(asset);
  },
  async pair(pair) {
    const assets = [pair.reference, pair.distorted];
    const entries = await session.entries(assets.map(asset => asset.key));
    for (let i = 0; i < assets.length; i++) requireEntry(entries[i], assets[i]);
    await prefetchEntries(entries.filter((_, i) => assets[i].ordinal === undefined), ITEM_BYTES);
    const reference = await fetchAsset(assets[0]);
    const distorted = await fetchAsset(assets[1]);
    return {pair, reference, distorted};
  },
  async entry(entryKey) {
    const [entry] = await session.entries([entryKey]);
    return entry;
  },
  stats() {
    return session.source.stats();
  },
};

let queue = Promise.resolve();
self.onmessage = ({data: {id, method, args}}) => {
  queue = queue.then(async () => {
    try {
      const result = await handlers[method](...args);
      const transfer = [];
      for (const item of Array.isArray(result) ? result : [result, result?.reference, result?.distorted]) {
        for (const data of [item?.bytes, item?.rgba, item?.pixels, item?.icc]) {
          if (ArrayBuffer.isView(data)) transfer.push(data.buffer);
        }
      }
      self.postMessage({id, result}, transfer);
    } catch (error) {
      self.postMessage({id, error: error.message || String(error)});
    }
  });
};
