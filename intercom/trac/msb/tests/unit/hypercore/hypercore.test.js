import { default as test } from 'brittle';

async function runHypercoreStallfixTests() {
    test.pause();
    await import('./status.test.js');
    await import('./stallfix.test.js');
    // the install-script tests need Node (fs, child processes)
    if (typeof Bare === 'undefined') await import('./apply.test.js');
    test.resume();
}

await runHypercoreStallfixTests();
