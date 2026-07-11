import test from 'node:test';
import assert from 'node:assert/strict';

import { dispatchContainedClientRequest } from '../features/sc-bridge/containment.js';

test('failed async client request is contained and the bridge serves the next request', async () => {
  const events = [];
  dispatchContainedClientRequest(
    async () => {
      throw new Error('injected session_open failure');
    },
    (error) => events.push(`error:${error.message}`)
  );
  await new Promise((resolve) => setTimeout(resolve, 0));
  dispatchContainedClientRequest(
    () => events.push('pong'),
    (error) => events.push(`error:${error.message}`)
  );

  assert.deepEqual(events, ['error:injected session_open failure', 'pong']);
});
