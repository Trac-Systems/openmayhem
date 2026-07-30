import { default as test } from 'brittle';

async function runV1ValidationSchemaTests() {
    test.pause();
    await import('./v1ValidationSchema/livenessRequest.test.js');
    await import('./v1ValidationSchema/livenessResponse.test.js');
    await import('./v1ValidationSchema/broadcastTransactionRequest.test.js');
    await import('./v1ValidationSchema/broadcastTransactionResponse.test.js');
    test.resume();
}

await runV1ValidationSchemaTests();

