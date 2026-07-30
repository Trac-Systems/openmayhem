// Aggregator for V1 handlers unit tests

import { default as test } from 'brittle';

async function runTests() {
    test.pause();

    await import('./handlers/BroadcastTransactionErrorMapper.test.js');
    await import('./handlers/BroadcastTransactionOperationStrategies.test.js');
    await import('./handlers/V1BaseOperationHandler.test.js');
    await import('./handlers/V1BroadcastTransactionOperationHandler.test.js');
    await import('./handlers/V1LivenessOperationHandler.test.js');

    test.resume();
}

await runTests();
