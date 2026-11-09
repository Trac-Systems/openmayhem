import { default as test } from 'brittle';

async function runNetworkModuleTests() {
    test.pause();
    await import('./Network.test.js');
    await import('./services/ConnectionManager.test.js');
    await import('./LegacyNetworkMessageRouter.test.js');
    await import('./ProtocolSession.test.js');
    await import('./shared/sharedModule.test.js');
    await import('./services/services.test.js');
    await import('./v1/v1.test.js');
    test.resume();
}

await runNetworkModuleTests();
