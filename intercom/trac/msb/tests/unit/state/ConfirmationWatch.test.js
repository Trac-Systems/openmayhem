import { test } from 'brittle';
import { ConfirmationWatch } from '../../../src/core/state/ConfirmationWatch.js';

function fakeBase({ confirmed, total, isIndexer = false, onForce = null }) {
    const base = {
        isIndexer,
        closing: null,
        forced: 0,
        fastForwarding: false,
        view: { core: { signedLength: confirmed, length: total } },
        isFastForwarding() { return this.fastForwarding },
        async forceFastForward() {
            this.forced++;
            if (onForce) onForce(this);
        }
    };
    return base;
}

function clock(start = 1_000_000) {
    const c = { t: start, now: () => c.t };
    return c;
}

const quiet = () => {};

test('ConfirmationWatch forces one fast-forward when the confirmed length is stuck behind local entries', async t => {
    // The Oct 31 shape: confirmed 964718, local 964869, nothing confirms any more.
    const base = fakeBase({
        confirmed: 964718,
        total: 964869,
        onForce: b => { b.view.core.signedLength = 964857; b.view.core.length = 964857; }
    });
    const c = clock();
    const watch = new ConfirmationWatch(base, { timeout: 120_000, now: c.now, log: quiet });

    t.is(await watch.check(), false, 'first sight only starts the clock');
    c.t += 119_000;
    t.is(await watch.check(), false, 'not yet');
    c.t += 1_000;
    t.is(await watch.check(), true, 'forced after the timeout');
    t.is(base.forced, 1);
    t.is(base.view.core.signedLength, 964857, 'caught up');

    c.t += 600_000;
    t.is(await watch.check(), false, 'nothing waiting after the catch-up');
    t.is(base.forced, 1);
});

test('ConfirmationWatch leaves a node alone while confirmations move', async t => {
    const base = fakeBase({ confirmed: 100, total: 110 });
    const c = clock();
    const watch = new ConfirmationWatch(base, { timeout: 120_000, now: c.now, log: quiet });
    for (let i = 0; i < 20; i++) {
        await watch.check();
        c.t += 60_000;
        base.view.core.signedLength += 1;
        base.view.core.length += 1;
    }
    t.is(base.forced, 0);
});

test('ConfirmationWatch leaves a quiet, caught-up node alone', async t => {
    const base = fakeBase({ confirmed: 500, total: 500 });
    const c = clock();
    const watch = new ConfirmationWatch(base, { timeout: 120_000, now: c.now, log: quiet });
    for (let i = 0; i < 10; i++) {
        await watch.check();
        c.t += 300_000;
    }
    t.is(base.forced, 0);
});

test('ConfirmationWatch never runs on an indexer', async t => {
    const base = fakeBase({ confirmed: 100, total: 200, isIndexer: true });
    const c = clock();
    const watch = new ConfirmationWatch(base, { timeout: 1_000, now: c.now, log: quiet });
    for (let i = 0; i < 5; i++) {
        await watch.check();
        c.t += 10_000;
    }
    t.is(base.forced, 0);
});

test('ConfirmationWatch waits while Autobase is fast-forwarding and restarts its clock', async t => {
    const base = fakeBase({ confirmed: 100, total: 200 });
    const c = clock();
    const watch = new ConfirmationWatch(base, { timeout: 120_000, now: c.now, log: quiet });
    await watch.check();
    c.t += 100_000;
    base.fastForwarding = true;
    t.is(await watch.check(), false);
    base.fastForwarding = false;
    c.t += 60_000;
    t.is(await watch.check(), false, 'clock restarted after the fast-forward');
    c.t += 120_000;
    t.is(await watch.check(), true);
    t.is(base.forced, 1);
});

test('ConfirmationWatch retries only after another full timeout and survives a failed fast-forward', async t => {
    const base = fakeBase({ confirmed: 100, total: 200 });
    base.forceFastForward = async function () { this.forced++; throw new Error('no peers') };
    const c = clock();
    const watch = new ConfirmationWatch(base, { timeout: 120_000, now: c.now, log: quiet });
    await watch.check();
    c.t += 120_000;
    t.is(await watch.check(), true);
    c.t += 60_000;
    t.is(await watch.check(), false, 'no immediate retry');
    c.t += 60_000;
    t.is(await watch.check(), true, 'retried after a full timeout');
    t.is(base.forced, 2);
});

test('ConfirmationWatch is off with a zero timeout', async t => {
    const watch = new ConfirmationWatch(fakeBase({ confirmed: 1, total: 2 }), { timeout: 0, log: quiet });
    t.is(watch.enabled, false);
    watch.start();
    watch.stop();
});
