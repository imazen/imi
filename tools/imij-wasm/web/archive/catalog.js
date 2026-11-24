export const PROFILE = 'imij.iqa/v1';
export const CATALOG_BYTES = 64 * 1024;
export const PAGE_BYTES = 1024 * 1024;
export const THUMBNAIL_BYTES = 512 * 1024;
export const ITEM_BYTES = 32 * 1024 * 1024;

const maxKey = (1n << 64n) - 1n;
const decoder = new TextDecoder('utf-8', {fatal: true});
const fail = message => { throw Error(`Invalid IQA catalog: ${message}`); };

function object(x) {
  return x !== null && typeof x === 'object' && !Array.isArray(x);
}
function string(x, max = 4096) {
  return typeof x === 'string' && x.length > 0 && x.length <= max;
}
function count(x, max) {
  return Number.isSafeInteger(x) && x > 0 && x <= max;
}
export function key(value) {
  if (typeof value !== 'string' || !/^(0|[1-9][0-9]{0,19})$/.test(value) ||
      BigInt(value) > maxKey) fail('entry key');
  return BigInt(value);
}
export function attribute(entry, name) {
  return entry?.attributes.find(value => value.name === name)?.value;
}
export function parseCatalog(bytes) {
  const root = JSON.parse(decoder.decode(bytes));
  if (root?.schema !== PROFILE || !string(root.dataset?.id, 128) || !string(root.dataset?.name) ||
      !object(root.dataset.metadata) || Object.entries(root.dataset.metadata).length > 64 ||
      !Object.entries(root.dataset.metadata).every(([name, value]) => string(name, 128) && string(value))) {
    fail('dataset declaration');
  }
  const pages = root.pairs;
  if (!count(pages?.pageCount, 1_000_000) || !count(pages.pageSize, 128) ||
      !count(pages.count, 128_000_000) || Math.ceil(pages.count / pages.pageSize) !== pages.pageCount ||
      key(pages.firstPage) === 0n || key(pages.firstPage) + BigInt(pages.pageCount - 1) > maxKey) {
    fail('pair page extent');
  }
  return root;
}
export function pageKey(catalog, index) {
  if (!Number.isInteger(index) || index < 0 || index >= catalog.pairs.pageCount) fail('page index');
  return key(catalog.pairs.firstPage) + BigInt(index);
}
function asset(item) {
  if (!object(item) || key(item.key) === 0n || !string(item.name) ||
      !string(item.mediaType, 127) ||
      (item.thumbnail !== undefined && key(item.thumbnail) === 0n)) fail('pair asset');
  if (item.ordinal !== undefined) key(item.ordinal);
  const widths = [16, 32, 64, 64];
  const digest = item.digest;
  if (!Number.isInteger(digest?.id) || digest.id < 0 || digest.id >= widths.length ||
      typeof digest.bytes !== 'string' || digest.bytes.length !== widths[digest.id] ||
      !/^[0-9a-f]+$/.test(digest.bytes)) fail('asset digest');
}
function score(item) {
  if (!object(item) || !['human', 'metric'].includes(item.kind) || !string(item.name, 128) ||
      !Number.isFinite(item.value) || !string(item.source) ||
      (item.version !== undefined && !string(item.version, 128)) ||
      (item.higherIsBetter !== undefined && typeof item.higherIsBetter !== 'boolean') ||
      ['min', 'max', 'std'].some(field => item[field] !== undefined && !Number.isFinite(item[field])) ||
      (item.min !== undefined && item.max !== undefined && item.min > item.max)) fail('score');
}
export function parsePage(bytes, catalog, index) {
  const page = JSON.parse(decoder.decode(bytes));
  const expected = Math.min(catalog.pairs.pageSize, catalog.pairs.count - index * catalog.pairs.pageSize);
  if (page?.schema !== PROFILE + '/page' || page.index !== index ||
      !Array.isArray(page.pairs) || page.pairs.length !== expected) fail('pair page');
  const ids = new Set();
  for (const pair of page.pairs) {
    if (!object(pair) || !string(pair.id, 128) || ids.has(pair.id) ||
        !Array.isArray(pair.scores) || pair.scores.length > 64 ||
        ['source', 'distortion', 'level', 'split'].some(field => pair[field] !== undefined && !string(pair[field]))) {
      fail('pair record');
    }
    ids.add(pair.id);
    asset(pair.reference);
    asset(pair.distorted);
    pair.scores.forEach(score);
  }
  return page.pairs;
}
