import test from 'brittle';
import Hypercore from 'hypercore';
import Corestore from 'corestore';
import { hypercoreStallfixStatus, hypercoreStallfixLine, HYPERCORE_STALLFIX_MARKER } from '../../../src/utils/hypercoreStallfix.js';

test('hypercoreStallfixStatus reports the loaded hypercore copy and sums counters over a store', async (t) => {
    const store = new Corestore(await t.tmp());
    t.teardown(() => store.close());
    const core = store.get({ name: 'stallfix-status' });
    await core.ready();
    const status = hypercoreStallfixStatus(core, store);
    t.is(status.patched, true, 'patched copy loaded');
    t.is(status.marker, HYPERCORE_STALLFIX_MARKER);
    t.is(status.expiredRequests, 0);
    core.core.replicator._sfStats.expired += 3;
    t.is(hypercoreStallfixStatus(core, store).expiredRequests, 3, 'summed from the store');
    core.core.replicator._sfStats.invalidOperationPauses += 2;
    t.is(hypercoreStallfixStatus(core, store).invalidOperationPauses, 2);
    t.ok(hypercoreStallfixLine('MSB', core).includes('active (openmayhem-stallfix-v1)'));
});

test('hypercoreStallfixStatus on an unpatched or missing core', (t) => {
    class Replicator {}
    const fake = { core: { replicator: new Replicator() } };
    t.alike(hypercoreStallfixStatus(fake), { patched: false, marker: null, expiredRequests: null, notAvailableSent: null, notAvailableSuppressed: null, invalidOperationPauses: null, invalidOperationPermanent: null });
    t.is(hypercoreStallfixStatus(null).patched, false);
    t.ok(hypercoreStallfixLine('MSB', fake).includes('NOT active'));
});

test('the loaded Hypercore class is the one autobase/corestore use', (t) => {
    t.is(typeof Hypercore, 'function');
});
