import test from 'brittle';

test.pause();
await import('./stallfix.test.js');
// the install-script tests need Node (fs, child processes)
if (typeof Bare === 'undefined') await import('./apply.test.js');
test.resume();
