import { test } from 'brittle';
import { ReplicationStallGuard } from '../../../../src/core/network/services/ReplicationStallGuard.js';

function fakeBase({ confirmed = 100, systemLength = 1000, systemSignedLength = 1000, isIndexer = false } = {}) {
    return {
        isIndexer,
        closing: null,
        closed: false,
        view: { core: { signedLength: confirmed, length: confirmed } },
        core: { length: systemLength, core: { state: { length: systemSignedLength } } }
    };
}

function fakeSwarm(n) {
    const swarm = { destroyed: false, suspended: false, connections: new Set(), refreshed: 0 };
    for (let i = 0; i < n; i++) {
        const c = {
            destroyed: false,
            explicit: i === 0, // a pinned/validator link is closed too
            destroy() { this.destroyed = true; swarm.connections.delete(this); }
        };
        swarm.connections.add(c);
    }
    return swarm;
}

function clock(start = 5_000_000) {
    const c = { t: start, now: () => c.t };
    return c;
}

function setup(baseOpts = {}, guardOpts = {}) {
    const base = fakeBase(baseOpts);
    const swarm = fakeSwarm(guardOpts.connections ?? 5);
    const c = clock();
    const logs = [];
    const guard = new ReplicationStallGuard(base, () => swarm, {
        timeout: 600_000,
        refresh: () => { swarm.refreshed++; },
        now: c.now,
        log: (line) => logs.push(line),
        ...guardOpts
    });
    return { base, swarm, c, logs, guard };
}

test('ReplicationStallGuard closes all connections once when signed system data stays unapplied and nothing confirms', t => {
    const { base, swarm, c, logs, guard } = setup({ confirmed: 1_091_699, systemLength: 3_551_909, systemSignedLength: 3_554_907 });
    const all = [...swarm.connections];

    t.is(guard.check(), false, 'first sight starts the clock');
    t.is(guard.status().state, 'behind');
    for (let i = 0; i < 39; i++) {
        c.t += 15_000;
        base.core.core.state.length += 10; // the network keeps moving
        t.is(guard.check(), false);
    }
    c.t += 15_000;
    t.is(guard.check(), true, 'acts after 10 min');
    t.ok(all.every((conn) => conn.destroyed), 'every connection closed, pinned one included');
    t.is(swarm.refreshed, 1, 'discovery refreshed once');
    const st = guard.status();
    t.is(st.state, 'stalled');
    t.is(st.reconnects, 1);
    t.is(st.lastReconnectClosed, 5);
    t.ok(logs.some((l) => l.includes('"event":"msb_replication_stalled"')), 'stalled event logged');
    t.ok(logs.some((l) => l.includes('"event":"msb_replication_reconnect_all"')), 'reconnect event logged');
    t.is(base.view.core.signedLength, 1_091_699, 'never changes state');
});

test('ReplicationStallGuard acts at most once per 10 minutes', t => {
    const { swarm, c, guard } = setup({ confirmed: 10, systemLength: 100, systemSignedLength: 200 });
    guard.check();
    c.t += 600_000;
    t.is(guard.check(), true, 'first reset');
    for (let i = 0; i < 3; i++) swarm.connections.add({ destroyed: false, destroy() { this.destroyed = true; swarm.connections.delete(this); } });
    c.t += 300_000;
    t.is(guard.check(), false, 'not before a new full window');
    c.t += 299_000;
    t.is(guard.check(), false, 'still within 10 min of the last reset');
    c.t += 1_000;
    t.is(guard.check(), true, 'second reset after 10 min');
    t.is(guard.status().reconnects, 2);
    t.is(swarm.connections.size, 0);
});

test('ReplicationStallGuard: no action when the node is not behind or confirms', t => {
    {
        const { swarm, c, guard } = setup({ confirmed: 10, systemLength: 100, systemSignedLength: 100 });
        for (let i = 0; i < 100; i++) { c.t += 15_000; t.is(guard.check(), false); }
        t.is(guard.status().state, 'ok', 'quiet network: nothing signed beyond the applied system length');
        t.is(swarm.connections.size, 5);
    }
    {
        const { base, swarm, c, guard } = setup({ confirmed: 10, systemLength: 100, systemSignedLength: 120 });
        for (let i = 0; i < 100; i++) {
            c.t += 15_000;
            base.view.core.signedLength++; // confirms
            base.core.length += 5;
            base.core.core.state.length += 5;
            t.is(guard.check(), false);
        }
        t.is(swarm.connections.size, 5, 'busy but healthy node keeps its connections');
    }
    {
        const { base, swarm, c, guard } = setup({ confirmed: 10, systemLength: 100, systemSignedLength: 120 });
        for (let i = 0; i < 100; i++) {
            c.t += 15_000;
            // catches up between checks every few rounds
            base.core.core.state.length = i % 4 === 0 ? base.core.length : base.core.length + 3;
            t.is(guard.check(), false);
        }
        t.is(swarm.connections.size, 5, 'intermittently ahead is not a stall');
    }
});

test('ReplicationStallGuard never runs on an indexer, when disabled, or when closing', t => {
    for (const [baseOpts, guardOpts, label] of [
        [{ isIndexer: true }, {}, 'indexer'],
        [{}, { timeout: 0 }, 'disabled'],
        [{}, { closing: true }, 'closing']
    ]) {
        const { base, swarm, c, guard } = setup({ confirmed: 10, systemLength: 100, systemSignedLength: 200, ...baseOpts }, guardOpts);
        if (guardOpts.closing) base.closing = Promise.resolve();
        guard.check();
        c.t += 3_600_000;
        t.is(guard.check(), false, label);
        t.is(swarm.connections.size, 5, label + ': connections kept');
    }
});

test('ReplicationStallGuard reports recovery when the confirmed length moves again', t => {
    const { base, c, logs, guard } = setup({ confirmed: 10, systemLength: 100, systemSignedLength: 200 });
    guard.check();
    c.t += 600_000;
    t.is(guard.check(), true);
    c.t += 15_000;
    base.view.core.signedLength = 11;
    t.is(guard.check(), false);
    t.is(guard.status().state, 'behind', 'still behind, but no longer stalled');
    t.ok(logs.some((l) => l.includes('"event":"msb_replication_recovered"')), 'recovered event logged');
    c.t += 15_000;
    base.core.length = 200;
    guard.check();
    t.is(guard.status().state, 'ok');
});

test('ReplicationStallGuard tolerates a missing swarm and odd lengths', t => {
    const base = fakeBase({ confirmed: 10, systemLength: 100, systemSignedLength: 200 });
    const c = clock();
    const guard = new ReplicationStallGuard(base, () => null, { timeout: 1_000, now: c.now, log: () => {} });
    guard.check();
    c.t += 1_000;
    t.is(guard.check(), true, 'acts, nothing to close');
    t.is(guard.status().lastReconnectClosed, 0);
    base.core.core.state.length = undefined;
    c.t += 5_000;
    t.is(guard.check(), false, 'no sample, no action');
});
