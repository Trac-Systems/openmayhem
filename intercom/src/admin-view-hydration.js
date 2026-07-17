import b4a from 'b4a';

const coreIdentity = (core, fallback) => {
  try {
    if (core?.key) return b4a.toString(core.key, 'hex');
  } catch {}
  return fallback;
};

export const joinCanonicalPeers = (peer, publicKeys) => {
  if (!Array.isArray(publicKeys) || publicKeys.length === 0) return 0;
  if (typeof peer?.swarm?.joinPeer !== 'function') {
    throw new Error('Canonical direct peers require an active Hyperswarm.');
  }
  let joined = 0;
  for (const publicKey of publicKeys) {
    const normalized = String(publicKey ?? '').trim().toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(normalized)) {
      throw new Error('Canonical direct peer must be a 32-byte public key.');
    }
    if (normalized === String(peer?.wallet?.publicKey ?? '').toLowerCase()) continue;
    peer.swarm.joinPeer(b4a.from(normalized, 'hex'));
    joined += 1;
  }
  return joined;
};

export const canonicalAdminViewCores = (peer) => {
  const applyState = peer?.base?._applyState;
  const candidates = [
    ['public', peer?.base?.view?.core],
    ['system', applyState?.system?.core],
    ...(Array.isArray(applyState?.views)
      ? applyState.views.map((entry, index) => [
        entry?.name || `view-${index}`,
        entry?.ref?.core || entry?.core,
      ])
      : []),
  ];
  const seen = new Set();
  const result = [];
  for (let index = 0; index < candidates.length; index += 1) {
    const [name, core] = candidates[index];
    if (!core || typeof core.download !== 'function') continue;
    const identity = coreIdentity(core, `object-${index}`);
    if (seen.has(identity)) continue;
    seen.add(identity);
    result.push({ name, core });
  }
  return result;
};

export async function hydrateAdminWriterViews(peer, { report = console.log } = {}) {
  const base = peer?.base;
  const admin = await base?.view?.get?.('admin');
  const isAdminWriter = base?.writable === true &&
    typeof admin?.value === 'string' &&
    admin.value === peer?.wallet?.publicKey;
  if (!isAdminWriter) return { required: false, views: [] };

  const views = canonicalAdminViewCores(peer);
  const hydrated = [];
  for (const { name, core } of views) {
    if (core.opened === false && typeof core.ready === 'function') await core.ready();
    const length = Number(core.length ?? 0);
    const before = Number(core.contiguousLength ?? 0);
    if (!Number.isSafeInteger(length) || length < 0 ||
        !Number.isSafeInteger(before) || before < 0 || before > length) {
      throw new Error(`Invalid canonical ${name} view availability.`);
    }
    if (before < length) {
      report(`Admin writer hydrating canonical ${name} view: ${before}/${length}`);
      const range = core.download({ start: 0, end: length, linear: true });
      try {
        await range.done();
      } finally {
        range.destroy();
      }
    }
    const after = Number(core.contiguousLength ?? 0);
    if (after < length) {
      throw new Error(`Canonical ${name} view is incomplete after hydration: ${after}/${length}.`);
    }
    hydrated.push({ name, length, before, after });
  }
  return { required: true, views: hydrated };
}
