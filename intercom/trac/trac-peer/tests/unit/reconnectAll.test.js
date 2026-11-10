import test from 'brittle';
import { createReconnectAll } from '../../src/network/reconnectAll.js';
import { createSubnetStallWatch } from '../../src/tasks/subnetStallWatch.js';
import { hypercoreStallfixStatus } from '../../src/utils/hypercoreStallfix.js';
import { Peer } from '../../src/index.js';
import Corestore from 'corestore';
import fs from 'fs';

function swarmWith(n) {
  const swarm = { destroyed: false, suspended: false, connections: new Set() };
  for (let i = 0; i < n; i++) {
    swarm.connections.add({ destroyed: false, pinned: i === 0,
      destroy() { this.destroyed = true; swarm.connections.delete(this); } });
  }
  return swarm;
}

test('subnet reconnect-all closes every connection once after a long stall, at most once per 10 min', t => {
  let time = 1000000, refreshed = 0;
  const logs = [];
  const swarm = swarmWith(4);
  const all = [...swarm.connections];
  const reconnect = createReconnectAll({ swarm, discovery: { refresh: async () => { refreshed++; } }, label: 'SUBNET',
    now: () => time, log: line => logs.push(line) });
  t.is(reconnect({ code: 'SUBNET_APPLY_STALLED', stalledMs: 60000 }), false, 'a 1 min stall only uses the one-peer retry');
  t.is(reconnect({ code: 'SUBNET_APPLY_STALLED', stalledMs: 600000 }), true, 'acts after 10 min');
  t.ok(all.every(c => c.destroyed), 'all connections closed, pinned included');
  t.is(reconnect.status().reconnects, 1);
  t.is(reconnect.status().lastReconnectClosed, 4);
  t.ok(logs[0].includes('"event":"reconnect_all"'));
  swarm.connections.add({ destroyed: false, destroy() { this.destroyed = true; swarm.connections.delete(this); } });
  time += 599999;
  t.is(reconnect({ stalledMs: 1200000 }), false, 'not again within 10 min');
  time += 1;
  t.is(reconnect({ stalledMs: 1200000 }), true, 'again after 10 min');
  t.is(swarm.connections.size, 0);
  return Promise.resolve().then(() => Promise.resolve()).then(() => t.is(refreshed, 2, 'discovery refreshed each time'));
});

test('subnet reconnect-all is off at 0, and leaves destroyed/suspended swarms alone', t => {
  const swarm = swarmWith(2);
  t.is(createReconnectAll({ swarm, label: 'SUBNET', afterMs: 0 })({ stalledMs: 99999999 }), false, 'disabled');
  swarm.suspended = true;
  t.is(createReconnectAll({ swarm, label: 'SUBNET', log: () => {} })({ stalledMs: 99999999 }), false, 'suspended');
  t.is(swarm.connections.size, 2);
  t.is(createReconnectAll({ swarm: null, label: 'SUBNET' })({ stalledMs: 99999999 }), false, 'no swarm');
  t.is(createReconnectAll({ swarm: swarmWith(1), label: 'SUBNET', log: () => {} })({ code: 'x' }), false, 'no stall duration');
});

test('subnet stall watch reports the time since the stall began and a status', t => {
  let time = 0;
  const events = [];
  const base = { opened: true, indexedLength: 100, core: { id: 'system', length: 100, core: { state: { length: 200 } } },
    view: { core: { length: 1, signedLength: 1 } }, _queueFastForward() {} };
  const watch = createSubnetStallWatch({ base, now: () => time, recover: e => events.push(e), log: () => {} });
  watch.check();
  t.is(watch.status().state, 'behind');
  time = 60000; t.is(watch.check(), true);
  t.is(events[0].stalledMs, 60000);
  t.is(watch.status().state, 'stalled');
  time = 600000; watch.check();
  t.is(events.at(-1).stalledMs, 600000, 'cumulative, not reset by each report');
  t.is(events.at(-1).elapsedMs, 540000);
  base.indexedLength++;
  base.core.length++;
  time = 615000; watch.check();
  t.is(watch.status().state, 'behind', 'progress ends the stall');
  base.core.core.state.length = base.core.length;
  time = 630000; watch.check();
  t.is(watch.status().state, 'ok');
});

test('trac-peer reports the loaded hypercore stall fix (subnet copy) and exports the helpers', async t => {
  const store = new Corestore(await t.tmp());
  t.teardown(() => store.close());
  const core = store.get({ name: 'x' });
  await core.ready();
  const status = hypercoreStallfixStatus(core, store);
  t.is(status.patched, true, 'subnet hypercore copy is patched');
  t.is(status.marker, 'openmayhem-stallfix-v2');
  t.is(typeof Peer.prototype.replicationStatus, 'function');
});

test('the subnet join..flush block stays byte-identical (downstream install-time patches anchor on it)', t => {
  // intercom-dex's postinstall patch "bounded-replicate-flush" replaces exactly this text; keep new code outside it.
  const source = fs.readFileSync(new URL('../../src/index.js', import.meta.url), 'utf8');
  const block = [
    "            const discovery = this.swarm.join(this.config.channel, { server: true, client: true });",
    "            this.recoverSubnet = createReadRecovery({",
    "                base: this.base, swarm: this.swarm, topic: this.config.channel,",
    "                discovery, label: 'SUBNET', rotate: false,",
    "            });",
    "            await this.swarm.flush();",
    ""
  ].join('\n');
  t.is(source.split(block).length, 2, 'block occurs exactly once');
});
