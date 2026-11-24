import assert from 'node:assert/strict';
import test from 'node:test';
import {rangeDemand} from '../archive/range-demand.js';

const MiB = 1024 * 1024;

function cache() {
  const fetched = new Map();
  return {
    fetched,
    source: {read(start, length) {
      return fetched.get(start) === start + length ? {start, length} : null;
    }},
    async ensure(spans) {
      for (const {start, end} of spans) fetched.set(start, end);
    },
  };
}

test('archive demand budgets successive reader misses within one operation', async () => {
  const size = 24 * MiB;
  const {fetched, source, ensure} = cache();
  const {read, demand} = rangeDemand(source, ensure);
  const item = (count) => {
    const output = [];
    for (let i = 0; i < count; i++) {
      const result = read(i * size, size);
      if (!result) throw Error('missing range');
      output.push(result);
    }
    return output;
  };
  await assert.rejects(demand(() => item(3)), /browser range budget/);
  assert.deepEqual([...fetched], [[0, size], [size, size * 2]]);
  assert.equal((await demand(() => item(3))).length, 3);
  assert.equal(fetched.get(size * 2), size * 3);
});

test('archive demand refuses a single oversized read before fetching', async () => {
  const {fetched, source, ensure} = cache();
  const {read, demand} = rangeDemand(source, ensure);
  await assert.rejects(demand(() => {
    if (!read(0, 33 * MiB)) throw Error('missing range');
  }), /browser range budget/);
  assert.equal(fetched.size, 0);
});
