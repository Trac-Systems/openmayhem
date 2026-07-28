import assert from 'node:assert/strict';
import test from 'node:test';
import { getStatePrefix, getStateValue } from '../src/rpc.js';

const entries = [
  { key: 'earn/fiat/a', value: { amount: 1 } },
  { key: 'earn/fiat/b', value: { amount: 2 } },
  { key: 'earn/fiat/c', value: { amount: 3 } },
  { key: 'other/key', value: { amount: 4 } },
];

function mockPeer({ signedLength = 9 } = {}) {
  const checkouts = [];
  let closed = 0;
  const makeView = () => ({
    async get(key) {
      return entries.find((entry) => entry.key === key)?.value ?? null;
    },
    createReadStream(options) {
      const filtered = entries
        .filter((entry) => options.gte == null || entry.key >= options.gte)
        .filter((entry) => options.gt == null || entry.key > options.gt)
        .filter((entry) => options.lt == null || entry.key < options.lt)
        .slice(0, options.limit);
      return (async function* read() {
        for (const entry of filtered) yield entry;
      })();
    },
    async close() {
      closed += 1;
    },
  });
  const live = makeView();
  live.core = { signedLength };
  live.checkout = (length) => {
    checkouts.push(length);
    return makeView();
  };
  return {
    peer: { base: { view: live } },
    checkouts,
    closed: () => closed,
  };
}

test('confirmed prefix paging pins signed length and advances strictly after the cursor', async () => {
  const state = mockPeer();
  const first = await getStatePrefix(state.peer, 'earn/fiat/', {
    confirmed: true,
    limit: 2,
  });
  assert.deepEqual(first, {
    prefix: 'earn/fiat/',
    confirmed: true,
    signed_length: 9,
    next_cursor: 'earn/fiat/b',
    truncated: true,
    values: entries.slice(0, 2),
  });

  const second = await getStatePrefix(state.peer, 'earn/fiat/', {
    confirmed: true,
    limit: 2,
    signedLength: first.signed_length,
    after: first.next_cursor,
  });
  assert.deepEqual(second.values, [entries[2]]);
  assert.equal(second.truncated, false);
  assert.equal(second.next_cursor, null);
  assert.deepEqual(state.checkouts, [9, 9]);
  assert.equal(state.closed(), 2);
});

test('confirmed exact reads use the requested checkout length', async () => {
  const state = mockPeer();
  const response = await getStateValue(state.peer, 'earn/fiat/b', {
    confirmed: true,
    signedLength: 7,
  });
  assert.equal(response.signed_length, 7);
  assert.deepEqual(response.value, { amount: 2 });
  assert.deepEqual(state.checkouts, [7]);
});

test('future and unconfirmed exact checkout requests fail closed', async () => {
  const state = mockPeer();
  await assert.rejects(
    getStatePrefix(state.peer, 'earn/fiat/', {
      confirmed: true,
      signedLength: 10,
    }),
    /confirmed length is 9/
  );
  await assert.rejects(
    getStatePrefix(state.peer, 'earn/fiat/', {
      confirmed: false,
      signedLength: 9,
    }),
    /exact checkout requires confirmed=true/
  );
});

test('cursor must remain inside the requested prefix', async () => {
  const state = mockPeer();
  await assert.rejects(
    getStatePrefix(state.peer, 'earn/fiat/', {
      confirmed: true,
      after: 'earn/tnk/a',
    }),
    /outside the requested prefix/
  );
});
