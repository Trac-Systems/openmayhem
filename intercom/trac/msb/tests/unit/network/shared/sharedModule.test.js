import { default as test } from 'brittle';

async function runSharedNetworkTests() {
    test.pause();
    await import('./validators/PartialOperationValidator.test.js');
    await import('./validators/PartialRoleAccessValidator.test.js');
    await import('./validators/PartialBootstrapDeploymentValidator.test.js');
    await import('./validators/PartialTransactionValidator.test.js');
    await import('./validators/PartialTransferValidator.test.js');
    test.resume();
}

await runSharedNetworkTests();
