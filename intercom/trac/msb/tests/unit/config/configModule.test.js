import { default as test } from 'brittle';

async function runConfigModuleTests() {
    test.pause();
    await import('./config.test.js');
    await import('./args.test.js');
    test.resume();
}

await runConfigModuleTests();
