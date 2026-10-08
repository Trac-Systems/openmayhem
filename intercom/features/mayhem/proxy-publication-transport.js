import b4a from 'b4a';
import crypto from 'crypto';
import { proxyRuntimeContext } from '../../contract/proxy-context.js';
import { PROXY_MAX_RECORD_BYTES } from '../../contract/proxy-protocol.js';
import { proxyPublicationFeatureKey, proxyPublicationParticipant } from '../../contract/proxy-publication.js';
import { validateProxyPendingEntry, ProxyPublicationJournal, ProxyPublicationController } from './proxy-publication-journal.js';
import { createProxyCanonicalSnapshot } from './proxy-canonical-view.js';

const hex = value => b4a.isBuffer(value) ? b4a.toString(value, 'hex') : String(value ?? '').toLowerCase();
const fail = message => { throw new Error(`Proxy publication recovery: ${message}.`); };
const terminal = value => value?.type === 'feature_result' && typeof value.ok === 'boolean';
const timeout = { timeout: 5000 };

// Isolate the pinned Autobase API assumptions here. Normal recovery reads exact
// result keys. Only ambiguous absence inspects the source interval AFTER this
// durable intent, incrementally; never old ledger history or all receipts.
const SOURCE_BLOCKS_PER_CHECK = 32;
export function createProxyPublicationTransport(peer, contractVersion) {
  const runtime = () => {
    const base = peer?.base;
    if (!base?.writable || !base.isIndexer || base.closing || base.paused ||
        peer?.contract?.instance?._mayhemReplayStatus?.active) fail('canonical writer is not ready');
    const local = base.local;
    const applied = base._applyState;
    if (!local || !applied?.view || !applied?.system || !base.view?.core) fail('source state is unavailable');
    const source = { writer_key: hex(local.key), fork: local.fork, length: local.length };
    if (!/^[0-9a-f]{64}$/.test(source.writer_key) || !Number.isSafeInteger(source.fork) || source.fork < 0 ||
        !Number.isSafeInteger(source.length) || source.length < 0) fail('source identity is invalid');
    return { base, local, applied, source };
  };
  const identity = () => {
    const { base, source } = runtime();
    const { epoch, ...context } = proxyRuntimeContext(peer, contractVersion, 0);
    if (hex(base.key) !== context.subnet_bootstrap) fail('configured subnet differs from writer');
    return { ...context, admin: hex(peer.wallet.publicKey), writer_key: source.writer_key, writer_fork: source.fork };
  };
  const featureKey = proxyPublicationFeatureKey;
  const signedHash = (envelope, nonce) => hex(peer.wallet.sign(`${JSON.stringify(envelope)}${nonce}`));

  const prepare = async (key, envelope, { fences } = {}) => {
    if (key !== await featureKey(envelope)) fail('feature key differs from signed operation');
    const { source } = runtime();
    const nonce = crypto.randomBytes(32).toString('hex');
    const hash = signedHash(envelope, nonce);
    const entry = { key, envelope: JSON.parse(JSON.stringify(envelope)), nonce, hash, result_key: `fr/${hash}`,
      scope: envelope.op === 'proxy_policy' ? 'admin:policy'
        : envelope.op === 'proxy_registry' ? `provider:${proxyPublicationParticipant(envelope)}`
          : `financial:${envelope.receipt?.body.accepted_terms ?? key.slice('proxy/spend/'.length)}`,
      ...(fences ? { fences: JSON.parse(JSON.stringify(fences)) } : {}),
      source: { ...source, checked_length: source.length, found_index: null }, created_at: Date.now() };
    validateProxyPendingEntry(entry);
    return entry;
  };

  const inspect = async entry => {
    validateProxyPendingEntry(entry);
    if (entry.key !== await featureKey(entry.envelope) || signedHash(entry.envelope, entry.nonce) !== entry.hash) {
      fail('saved envelope/nonce/result binding differs');
    }
    const before = runtime();
    if (before.source.writer_key !== entry.source.writer_key || before.source.fork !== entry.source.fork ||
        before.source.length < entry.source.length) fail('source identity regressed or changed');
    const signedLength = before.base.view.core.signedLength;
    const signed = before.base.view.checkout(signedLength);
    try {
      await signed.ready();
      if (hex(signed.core.key) !== hex(before.applied.view.core.key) ||
          signed.core.fork !== before.applied.view.core.fork ||
          !b4a.equals(await signed.core.treeHash(signedLength), await before.applied.view.core.treeHash(signedLength))) {
        fail('signed prefix differs from canonical applied state');
      }
      if ((await signed.get('admin', timeout))?.value !== hex(peer.wallet.publicKey)) fail('local identity is not canonical admin');
      const result = (await signed.get(entry.result_key, timeout))?.value;
      if (terminal(result)) {
        if (result.feature_key !== entry.key || result.hash !== entry.hash || result.address !== hex(peer.wallet.publicKey)) {
          fail('canonical result identity differs');
        }
        return { state: 'confirmed', result };
      }
    } finally { await signed.close(); }

    // An outstanding base.append may still commit later even if its caller stopped
    // waiting. Never interpret a timeout, ACK loss or unflushed queue as absence.
    // _advancing retains the last resolved promise in the pinned Autobase version;
    // _draining is the actual active-application flag. Recheck source/view identity
    // around each await as well, rather than assuming that promise means busy.
    const idle = value => value.base._appending === null && value.base._draining === false &&
      !value.base.isFastForwarding() && value.base.localWriter?.idle() === true;
    if (!idle(before)) return { state: 'pending', reason: 'writer_active' };
    const view = before.applied.view;
    const length = view.core.length;
    const fork = view.core.fork;
    const key = hex(view.core.key);
    const [appliedResult, consumed, progress] = await Promise.all([
      view.get(entry.result_key, timeout), view.get(`sh/${entry.hash}`, timeout),
      before.applied.system.get(before.local.key, timeout),
    ]);
    const after = runtime();
    if (after.base !== before.base || after.applied !== before.applied || after.local !== before.local ||
        after.source.writer_key !== before.source.writer_key || after.source.fork !== before.source.fork ||
        after.source.length !== before.source.length || view.core.length !== length || view.core.fork !== fork ||
        hex(view.core.key) !== key || !idle(after)) return { state: 'pending', reason: 'source_advanced' };
    if (appliedResult !== null || consumed !== null || entry.source.found_index !== null) {
      return { state: 'pending', reason: 'awaiting_canonical_result' };
    }
    if (!progress || progress.isRemoved || !Number.isSafeInteger(progress.length) || progress.length < after.source.length) {
      return { state: 'pending', reason: 'source_not_applied' };
    }
    // Missing fr alone does not prove no append: generic feature validation can
    // discard a source block before recording a result. Inspect only this pending
    // intent's source interval and persist progress; one check reads <=32 blocks.
    const source = { ...entry.source };
    const end = Math.min(after.source.length, source.checked_length + SOURCE_BLOCKS_PER_CHECK);
    for (let index = source.checked_length; index < end; index++) {
      const block = await after.local.get(index, timeout);
      if (!block || !block.node) fail('source evidence is unavailable');
      const bytes = block.node.value;
      // Our bounded serialized operation cannot be one of these larger frames.
      if (bytes && bytes.byteLength <= PROXY_MAX_RECORD_BYTES + 4096) {
        const operation = JSON.parse(b4a.toString(bytes));
        if (operation?.type === 'feature' && operation?.value?.dispatch?.hash === entry.hash) {
          source.found_index = index;
        }
      }
      source.checked_length = index + 1;
      if (source.found_index !== null) break;
    }
    const final = runtime();
    if (final.source.writer_key !== after.source.writer_key || final.source.fork !== after.source.fork ||
        final.source.length < source.checked_length) fail('source changed during recovery');
    if (source.found_index !== null || source.checked_length < final.source.length || !idle(final)) {
      return { state: 'pending', reason: source.found_index !== null ? 'awaiting_canonical_result' : 'source_check_pending', source };
    }
    // No source block for this nonce exists and no old append can still run.
    return { state: 'absent', source };
  };
  return { identity, prepare, inspect };
}

export async function installProxyPublicationController(feature, { directory, contractVersion, maxEntries, recoveryIntervalMs = 5000 }) {
  if (feature.proxyPublicationController) fail('controller is already configured');
  if (!Number.isSafeInteger(recoveryIntervalMs) || recoveryIntervalMs < 1000) fail('invalid recovery interval');
  const transport = createProxyPublicationTransport(feature.peer, contractVersion);
  const journal = await ProxyPublicationJournal.open({ directory, identity: transport.identity(), maxEntries });
  try {
    // Validate saved local entries before any activation or background work.
    // No forward dispatch here: startup recovery is subject to fresh admission.
    for (const entry of journal.list()) {
      validateProxyPendingEntry(entry);
      const hash = hex(feature.peer.wallet.sign(`${JSON.stringify(entry.envelope)}${entry.nonce}`));
      const key = await proxyPublicationFeatureKey(entry.envelope);
      if (key !== entry.key || hash !== entry.hash) fail('saved publication binding is invalid');
    }
    const controller = new ProxyPublicationController({ journal, ...transport,
      maxInFlight: Math.min(16, journal.maxEntries),
      admit: (key, envelope, forward) => feature._admitProxyPublication(key, envelope, forward),
      append: entry => feature._submitFeature(entry.key, entry.envelope, { nonce: entry.nonce }),
      result: (entry, result) => feature._featureResponse(entry.key, entry.hash, entry.result_key, result),
    });
    feature.withProxyCanonicalSnapshot = createProxyCanonicalSnapshot(feature.peer, contractVersion);
    feature.proxyPublicationController = controller;
    controller.timer = setInterval(() => { controller.step().catch(() => {}); }, recoveryIntervalMs);
    controller.timer.unref?.();
    return controller;
  } catch (error) { await journal.close(); throw error; }
}
