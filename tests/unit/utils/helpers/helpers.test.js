import { default as test } from 'brittle';

async function runMsgUtilsTests() {
    test.pause();

    await import('./isHexString.test.js');
    await import('./normalizeHex.test.js');
    test.resume();
}

runMsgUtilsTests();