import { default as test } from 'brittle';

async function runNetworkModuleTests() {
    test.pause();
    await import('./ConnectionManager.test.js');
    await import('./NetworkWalletFactory.test.js');
    test.resume();
}

await runNetworkModuleTests();
