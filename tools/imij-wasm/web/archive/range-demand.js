import {mergeAdjacent} from '../gigapixel/prefetch.js';

const MiB = 1024 * 1024;
export const ARCHIVE_CACHE_BYTES = 64 * MiB;
const MAX_STEP_BYTES = 32 * MiB;
const MAX_STEPS = 128;

// The archive worker serializes operations. Each retry replays the positional
// reader against its cache and requests the ranges that were still missing.
export function rangeDemand(source, ensure) {
  let missing;
  function read(start, length) {
    const cached = source.read(start, length, false);
    if (!cached) missing.push({start, end: start + length});
    return cached;
  }
  function readMany(pairs) {
    const data = [];
    for (let i = 0; i < pairs.length; i += 2) data.push(read(pairs[i], pairs[i + 1]));
    return data;
  }
  async function demand(action) {
    let remaining = ARCHIVE_CACHE_BYTES;
    for (let step = 0; step < MAX_STEPS; step++) {
      missing = [];
      try {
        return action();
      } catch (error) {
        if (missing.length === 0) throw error;
        const spans = mergeAdjacent(missing);
        const requested = spans.reduce((sum, {start, end}) => sum + end - start, 0);
        if (requested > MAX_STEP_BYTES || requested > remaining) {
          throw Error('Archive operation exceeds the browser range budget.');
        }
        remaining -= requested;
        await ensure(spans);
      }
    }
    throw Error('Archive operation exceeds the dependent-read budget.');
  }
  return {read, readMany, demand};
}
