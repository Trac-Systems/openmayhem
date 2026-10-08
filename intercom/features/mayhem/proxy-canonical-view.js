// Publication-only reads from the canonical admin indexer. Readers never use
// this adapter: their signed local view is not proof of the current canonical
// head. They obtain authenticated preflight from the indexer service instead.
import b4a from 'b4a';
import { proxyRuntimeContext } from '../../contract/proxy-context.js';
import { PROXY_MAX_RECORD_BYTES } from '../../contract/proxy-protocol.js';

const hex = value => b4a.isBuffer(value) ? b4a.toString(value, 'hex') : String(value ?? '').toLowerCase();
const clone = value => value === null ? null : JSON.parse(JSON.stringify(value));
const fail = message => { throw new Error(`Proxy canonical snapshot unavailable: ${message}.`); };
const READ_TIMEOUT_MS = 5_000;
// Bounds apply to one registry mutation, not model context, catalog size or
// inference. A mutation reads only the exact keys required by its wire schema.
const MAX_KEYS = 128;
const MAX_READ_BYTES = PROXY_MAX_RECORD_BYTES * MAX_KEYS;

export function createProxyCanonicalSnapshot(peer, contractVersion) {
  const runtime = () => {
    const base = peer?.base;
    const core = base?.view?.core;
    const applied = base?._applyState?.view?.core;
    if (base?.writable !== true || base?.isIndexer !== true || base.closing || base.paused ||
        peer?.contract?.instance?._mayhemReplayStatus?.active === true) fail('canonical indexer is not ready');
    if (!core || !applied || core.closing || applied.closing ||
        typeof core.treeHash !== 'function' || typeof applied.treeHash !== 'function') fail('canonical view is missing');
    const length = core.signedLength;
    if (!Number.isSafeInteger(length) || length < 1 || applied.length < length) fail('confirmed state is not materialized');
    const key = hex(core.key);
    const fork = core.fork;
    const admin = hex(peer?.wallet?.publicKey);
    if (!/^[0-9a-f]{64}$/.test(key) || !/^[0-9a-f]{64}$/.test(admin) ||
        !Number.isSafeInteger(fork) || fork < 0) fail('canonical identity is invalid');
    return { base, core, applied, length, key, fork, admin };
  };

  const pin = async () => {
    const before = runtime();
    const [viewHash, appliedHash] = await Promise.all([
      before.core.treeHash(before.length), before.applied.treeHash(before.length),
    ]);
    const after = runtime();
    if (after.base !== before.base || after.core !== before.core || after.applied !== before.applied ||
        after.key !== before.key || after.fork !== before.fork || after.admin !== before.admin ||
        after.length < before.length || !b4a.equals(viewHash, appliedHash)) fail('read view differs from canonical applied prefix');
    const view = before.base.view.checkout(before.length);
    try {
      await view.ready();
      if (hex(view.core.key) !== before.key || view.core.fork !== before.fork) fail('checkout identity changed');
      const read = async key => clone((await view.get(key, { timeout: READ_TIMEOUT_MS }))?.value ?? null);
      if (await read('admin') !== before.admin) fail('local identity is not the canonical admin');
      const epoch = (await read('epoch/apply/state'))?.epoch ?? 0;
      const context = proxyRuntimeContext(peer, contractVersion, epoch);
      if (hex(before.base.key) !== context.subnet_bootstrap) fail('configured bootstrap differs from canonical indexer');
      return { ...before, view, read, context,
        proof: { view_key: before.key, fork: before.fork, signed_length: before.length, tree_hash: hex(viewHash) } };
    } catch (error) {
      await view.close();
      throw error;
    }
  };

  return async body => {
    const initial = await pin();
    const observed = new Map();
    let readBytes = 0;
    try {
      const read = async key => {
        if (typeof key !== 'string' || key.length > 256 || !key.startsWith('proxy/v1/')) fail('invalid registry read key');
        if (observed.has(key)) return clone(observed.get(key));
        if (observed.size >= MAX_KEYS) fail('registry read count exceeds its bound');
        const value = await initial.read(key);
        readBytes += b4a.byteLength(JSON.stringify(value));
        if (readBytes > MAX_READ_BYTES) fail('registry read bytes exceed their bound');
        observed.set(key, value);
        return clone(value);
      };
      const assertCurrent = async () => {
        const current = await pin();
        try {
          if (current.key !== initial.key || current.fork !== initial.fork || current.length < initial.length ||
              current.admin !== initial.admin || JSON.stringify(current.context) !== JSON.stringify(initial.context)) {
            fail('canonical context changed');
          }
          // Native activity may advance the head. Compare only keys this
          // admission actually consulted; never rescan a prefix/history.
          if (current.length !== initial.length) {
            for (const [key, value] of observed) {
              if (JSON.stringify(await current.read(key)) !== JSON.stringify(value)) fail('registry state changed during validation');
            }
          }
        } finally { await current.view.close(); }
      };
      return await body({ context: initial.context, proof: initial.proof, read, assertCurrent });
    } finally { await initial.view.close(); }
  };
}
