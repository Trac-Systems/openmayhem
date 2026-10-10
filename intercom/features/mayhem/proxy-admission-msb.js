// Read-only signed MSB identity exposed only through the authenticated canonical
// admission service. This is observer freshness, never a ledger payment time.
import b4a from 'b4a';
const hex = v => typeof v === 'string' && /^[0-9a-f]{64}$/.test(v);
const uint = (v, min = 0) => Number.isSafeInteger(v) && v >= min;
const asHex = v => b4a.isBuffer(v) ? b4a.toString(v, 'hex') : null;
const need = (ok, message) => { if (!ok) throw new Error(`Proxy admission MSB: ${message}.`); };

export function validateAdmissionMsbSnapshot(value, context, now = Date.now()) {
  need(value && Object.keys(value).sort().join('|') === 'fork|msb_bootstrap|network_id|observed_at_ms|signed_length|tree_hash|view_key'
    && value.network_id === context.network_id && value.msb_bootstrap === context.msb_bootstrap
    && hex(value.view_key) && hex(value.tree_hash) && hex(value.msb_bootstrap) && uint(value.fork)
    && uint(value.signed_length, 1) && uint(value.observed_at_ms, 1)
    && Math.abs(now - value.observed_at_ms) <= 15000, 'foreign, stale or invalid signed snapshot');
  return value;
}

export function createAdmissionMsbReader(msb, { now = Date.now, timeoutMs = 4000 } = {}) {
  need(uint(timeoutMs, 1) && timeoutMs <= 4000, 'invalid read deadline');
  let active = 0;
  return async context => {
    need(active < 4, 'read capacity busy'); active++;
    let timer, snapshot;
    const work = Promise.resolve().then(async () => {
      const base = msb.state?.base, view = base?.view, core = view?.core;
      const length = msb.state?.getSignedLength(), key = asHex(core?.key), fork = core?.fork;
      const network = { network_id: String(msb.config?.networkId), msb_bootstrap: asHex(msb.config?.bootstrap) };
      const online = () => msb.state?.isIndexer?.() === true || (msb.network?.validatorConnectionManager?.connectionCount?.() ?? 0) > 0;
      need(core && hex(key) && uint(fork) && uint(length, 1) && typeof view.checkout === 'function'
        && network.network_id === context.network_id && network.msb_bootstrap === context.msb_bootstrap && online(), 'signed source unavailable');
      const stable = () => need(msb.state.base === base && base.view === view && view.core === core
        && asHex(core.key) === key && core.fork === fork && msb.state.getSignedLength() >= length && online()
        && String(msb.config.networkId) === network.network_id && asHex(msb.config.bootstrap) === network.msb_bootstrap,
      'signed source changed');
      snapshot = view.checkout(length);
      need(typeof snapshot.core?.treeHash === 'function', 'signed snapshot hash unavailable');
      const treeHash = asHex(await snapshot.core.treeHash(length)); stable();
      return validateAdmissionMsbSnapshot({ ...network, view_key: key, fork, signed_length: length,
        tree_hash: treeHash, observed_at_ms: now() }, context, now());
    }).finally(async () => {
      clearTimeout(timer);
      try { await snapshot?.close(); } finally { active--; }
    });
    const expired = new Promise((_, reject) => {
      timer = setTimeout(() => {
        // Closing the owned snapshot cancels sparse reads; an unresolved read
        // retains its permit until real cleanup, bounding repeated timeouts.
        void snapshot?.close().catch(() => {});
        reject(new Error('Proxy admission MSB: signed read deadline exceeded.'));
      }, timeoutMs);
    });
    return Promise.race([work, expired]);
  };
}
