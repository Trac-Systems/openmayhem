import b4a from 'b4a';

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

export async function hydrateAdminWriterViews(peer, { report = console.log } = {}) {
  const base = peer?.base;
  const view = base?.view;
  const admin = await view?.get?.('admin');
  const isAdminWriter = base?.writable === true &&
    typeof admin?.value === 'string' &&
    admin.value === peer?.wallet?.publicKey;
  if (!isAdminWriter) return { required: false, entries: 0 };
  if (typeof view?.createReadStream !== 'function') {
    throw new Error('Canonical admin view cannot be traversed.');
  }

  const core = view.core;
  const length = Number(core?.length ?? 0);
  const contiguous = Number(core?.contiguousLength ?? 0);
  report(`Admin writer hydrating canonical state: view ${contiguous}/${length}`);
  let entries = 0;
  for await (const _entry of view.createReadStream()) {
    entries += 1;
  }
  report(`Admin writer canonical state ready: ${entries} entries`);
  return { required: true, entries, length, contiguous };
}
