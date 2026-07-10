import assert from 'node:assert/strict';
import test from 'node:test';

import { runRootMsbTransferHelper } from '../src/msb-transfer-helper.js';

test('root Intercom app resolves the bundled MSB transfer helper', async () => {
  await assert.rejects(
    runRootMsbTransferHelper('invalid'),
    /Usage:/
  );
});
