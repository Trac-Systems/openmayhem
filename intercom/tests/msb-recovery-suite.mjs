const {default:test}=await import('brittle');test.configure({timeout:60000});test.pause();
await import('../trac/trac-peer/tests/unit/msbReadBounds.test.js');
await import('../trac/msb/tests/unit/state/ConfirmationWatch.test.js');
await import('../trac/msb/tests/unit/network/ReadRecovery.test.js');
const done=test('sidecar suite completed',t=>t.pass());test.resume();await done;
if(globalThis.Bare){Bare.emit('beforeExit',0);Bare.exit(Bare.exitCode??0)}
