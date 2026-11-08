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

test('ConfirmationWatch preserves stalled progress while fast-forwarding', async t => {
    const base = fakeBase({ confirmed: 100, total: 200 });
    const c = clock();
    const events = [];
    const watch = new ConfirmationWatch(base, { timeout: 120_000, now: c.now, log: quiet, onStall: e => events.push(e) });
    await watch.check();
    c.t += 100_000;
    base.fastForwarding = true;
    t.is(await watch.check(), false);
    c.t += 60_000;
    t.is(await watch.check(), false, 'does not overlap fast-forward operations');
    t.is(events.length, 1, 'transport recovery remains active');
    t.is(events[0].msbsl, 200);
    base.fastForwarding = false;
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

test('normal return without progress is not reported as success and requests recovery', async t => {
    const c = clock(), logs = [], events = [];
    const base = fakeBase({ confirmed: 1, total: 2 });
    const watch = new ConfirmationWatch(base, { timeout: 10, now: c.now, log: x => logs.push(x), onStall: x => events.push(x) });
    await watch.check(); c.t += 10; await watch.check();
    t.is(events.length, 1); t.is(events[0].outcome, 'no_progress');
    t.not(logs.some(x => x.includes('finished')), true);
    base.forceFastForward = async () => { base.fastForwardFailedAt = c.t; };
    c.t += 10; await watch.check(); t.is(events[1].outcome, 'failed');
    base.forceFastForward = async () => { base.view.core.signedLength++; };
    c.t += 10; await watch.check(); t.is(events.length, 2, 'actual advancement does not repair transport');
    t.ok(logs.some(x => x.includes('"outcome":"advanced"')));
});

test('in-flight fast-forward is single-flight and stop suppresses late recovery', async t => {
    const c = clock(); let finish, recovered = 0;
    const base = fakeBase({ confirmed: 1, total: 2 });
    base.forceFastForward = () => new Promise(r => { finish = r; });
    const watch = new ConfirmationWatch(base, { timeout: 10, now: c.now, log: quiet, onStall: () => recovered++ });
    await watch.check(); c.t += 10; const pending = watch.check();
    t.is(await watch.check(), false); t.is(recovered, 1, 'pending work requests transport recovery');
    watch.stop(); finish(); await pending;
    t.is(recovered, 1, 'shutdown suppresses any late recovery'); t.is(await watch.check(), false);
});

test('known signed system head recovers when local view has no unconfirmed entries', async t => {
    const c=clock(),events=[],base=fakeBase({confirmed:10,total:10});
    base.core={length:20,core:{state:{length:40}}};
    const watch=new ConfirmationWatch(base,{timeout:10,now:c.now,log:quiet,onStall:e=>events.push(e)});
    await watch.check();c.t+=10;await watch.check();t.is(base.forced,1);t.is(events.length,1);
});

test('pending force operation keeps requesting transport recovery without overlapping force calls', async t => {
    const c=clock(),events=[],base=fakeBase({confirmed:10,total:11});let finish;
    base.forceFastForward=()=>{base.forced++;return new Promise(r=>{finish=r})};
    const watch=new ConfirmationWatch(base,{timeout:10,now:c.now,log:quiet,onStall:e=>events.push(e)});
    await watch.check();c.t+=10;const pending=watch.check();c.t+=10;await watch.check();
    t.is(base.forced,1);t.is(events.length,1);t.is(events[0].msbsl,11);
    watch.stop();finish();await pending;t.is(events.length,1,'no late callback after shutdown');
});

test('confirmed progress while force is pending clears the mark safely', async t => {
    const c=clock(),events=[],base=fakeBase({confirmed:10,total:11});let finish;
    base.forceFastForward=()=>new Promise(r=>{finish=r});
    const watch=new ConfirmationWatch(base,{timeout:10,now:c.now,log:quiet,onStall:e=>events.push(e)});
    await watch.check();c.t+=10;const pending=watch.check();
    base.view.core.signedLength=11;await watch.check();finish();await pending;
    t.is(events.length,0,'no recovery after genuine progress');watch.stop();
});
