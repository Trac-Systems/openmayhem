/**
 * Integration tests for getActiveWriterCount cache invalidation.
 *
 * Unit tests stub system.list and system.core is undefined there (systemLength = -1),
 * so the cache never hits in unit tests. These tests validate two things with real
 * Autobase instances:
 *
 *   1. The cache invalidation invariant: system.core.length actually increments when
 *      Autobase processes writes, so the cache key is semantically correct.
 *
 *   2. getActiveWriterCount works end-to-end on a real State+Autobase, where
 *      system.core.length is a real number and the cache path is exercised.
 */

import { test } from 'brittle';
import sinon from 'sinon';
import { createStores, createBase, defaultOpenHyperbeeView } from '../helpers/autobaseTestHelpers.js';
import { setupStateNetwork } from '../helpers/StateNetworkFactory.js';
import { AUTOBASE_VALUE_ENCODING, ACK_INTERVAL } from '../../src/utils/constants.js';

const BASE_OPTS = {
    open: defaultOpenHyperbeeView,
    valueEncoding: AUTOBASE_VALUE_ENCODING,
    ackInterval: ACK_INTERVAL,
    bigBatches: false,
    optimistic: false,
};

function createHarness() {
    const teardowns = [];
    return {
        teardown(fn, opts = {}) { teardowns.push({ fn, order: opts.order ?? 0 }); },
        async cleanup() {
            const sorted = teardowns.sort((a, b) => b.order - a.order);
            for (const { fn } of sorted) {
                try { await fn(); } catch (err) { console.error('teardown error', err); }
            }
        }
    };
}

test('system.core.length increments after append+advance — the invariant the getActiveWriterCount cache relies on', async t => {
    const harness = createHarness();
    t.teardown(() => harness.cleanup());

    const [store] = await createStores(1, harness);
    const base = createBase(store, null, harness, BASE_OPTS);
    await base.ready();

    t.ok(typeof base.system?.core?.length === 'number', 'system.core.length is a real number on a live Autobase');

    const lengthBefore = base.system.core.length;

    await base.append('test-entry');
    await base.advance();

    const lengthAfter = base.system.core.length;
    t.ok(lengthAfter > lengthBefore, `system.core.length increments after a write (${lengthBefore} → ${lengthAfter})`);
});

test('getActiveWriterCount uses system.core.length as cache key on a real State — cache hits and misses fire correctly', async t => {
    const network = await setupStateNetwork({ nodes: 2 });
    t.teardown(() => network.teardown());

    const admin = network.adminBootstrap;
    await admin.state.ready();

    t.ok(typeof admin.state.base.system?.core?.length === 'number', 'state.base.system.core.length is accessible');

    // Spy on system.list to count real traversals (not mocking the return value).
    const listSpy = sinon.spy(admin.state.base.system, 'list');
    t.teardown(() => listSpy.restore());

    // First call must traverse system.list (cache is empty).
    await admin.state.getActiveWriterCount();
    t.is(listSpy.callCount, 1, 'first call traverses system.list (cache cold)');

    // Second call with the same systemLength must hit the cache.
    await admin.state.getActiveWriterCount();
    t.is(listSpy.callCount, 1, 'second call returns cached value without re-traversing system.list');

    // The excludeAdmin=true variant has its own cache slot and must also traverse once then cache.
    await admin.state.getActiveWriterCount(true);
    t.is(listSpy.callCount, 2, 'excludeAdmin=true cold-starts its own cache slot');

    await admin.state.getActiveWriterCount(true);
    t.is(listSpy.callCount, 2, 'excludeAdmin=true second call hits cache');
});
