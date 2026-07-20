import http from 'http';
import b4a from 'b4a';
import { applyCors } from '../trac/trac-peer/rpc/cors.js';
import { DEFAULT_MAX_BODY_BYTES } from '../trac/trac-peer/rpc/constants.js';
import { routes } from '../trac/trac-peer/rpc/routes/index.js';
import { readJsonBody } from '../trac/trac-peer/rpc/utils/body.js';

const isObject = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const normalizeKey = (value) => String(value ?? '').trim().toLowerCase();

const stateView = (peer, confirmed) => {
  if (!peer.base?.view) throw new Error('Peer view not ready.');
  if (!confirmed) return { view: peer.base.view, close: async () => {} };
  const view = peer.base.view.checkout(peer.base.view.core.signedLength);
  return { view, close: async () => view.close() };
};

export async function getStatePrefix(peer, prefix, { confirmed = true, limit = 500 } = {}) {
  const normalizedPrefix = String(prefix ?? '');
  if (!normalizedPrefix) throw new Error('Missing prefix.');
  if (normalizedPrefix.length > 256) throw new Error('Prefix is too long.');
  const normalizedLimit = Number(limit);
  if (!Number.isInteger(normalizedLimit) || normalizedLimit < 1 || normalizedLimit > 1000) {
    throw new Error('Invalid limit. Expected an integer from 1 to 1000.');
  }

  const session = stateView(peer, confirmed);
  const values = [];
  try {
    const stream = session.view.createReadStream({
      gte: normalizedPrefix,
      lt: `${normalizedPrefix}\xff`,
      limit: normalizedLimit,
    });
    for await (const entry of stream) {
      values.push({ key: entry.key, value: entry.value });
    }
  } finally {
    await session.close();
  }
  return { prefix: normalizedPrefix, confirmed, values };
}

export async function getMayhemStatus(peer, metadata = {}) {
  const peerMsbAddress = peer.msbClient.pubKeyHexToAddress(peer.wallet.publicKey);
  const admin = peer.base?.view ? await peer.base.view.get('admin') : null;
  const chatStatus = peer.base?.view ? await peer.base.view.get('chat_status') : null;
  const fallbackBootstrap = peer.base?.key ?? peer.config?.bootstrap ?? peer.bootstrap;
  const subnetBootstrapHex = metadata.subnetBootstrapHex ??
    (b4a.isBuffer(fallbackBootstrap)
      ? b4a.toString(fallbackBootstrap, 'hex')
      : fallbackBootstrap != null
        ? String(fallbackBootstrap)
        : null);
  return {
    peer: {
      pubKeyHex: peer.wallet?.publicKey ?? null,
      writerKeyHex: peer.writerLocalKey ?? null,
      msbAddress: peerMsbAddress,
      baseWritable: peer.base?.writable === true,
      isIndexer: peer.base?.isIndexer === true,
      isWriter: peer.base?.writable === true,
      subnetBootstrapHex,
      subnetChannelUtf8: metadata.subnetChannelUtf8 ?? peer.config?.channelName ?? null,
      dhtBootstrap: Array.isArray(metadata.peerDhtBootstrap)
        ? metadata.peerDhtBootstrap
        : Array.isArray(peer.config?.dhtBootstrap)
          ? peer.config.dhtBootstrap
          : [],
      subnetSignedLength: peer.base?.view?.core?.signedLength ?? null,
      subnetUnsignedLength: peer.base?.view?.core?.length ?? null,
      admin: admin?.value ?? null,
      chatStatus: chatStatus?.value ?? null,
    },
    msb: {
      ready: true,
      bootstrapHex: peer.msbClient.bootstrapHex,
      channel: metadata.msbChannel ?? null,
      networkId: peer.msbClient.networkId,
      signedLength: peer.msbClient.getSignedLength(),
      connectedValidators: peer.msbClient.getConnectedValidatorsCount?.() ?? 0,
      dhtBootstrap: Array.isArray(metadata.msbDhtBootstrap) ? metadata.msbDhtBootstrap : [],
    },
  };
}

export const loadPrivateInternalAuthSecret = ({
  fsModule,
  pathModule,
  secretPath,
  platform = typeof process !== 'undefined'
    ? process.platform
    : typeof Bare !== 'undefined'
      ? Bare.platform
      : '',
}) => {
  const configuredPath = String(secretPath ?? '').trim();
  if (!configuredPath) {
    throw new Error('MAYHEM_PAYGATE_INTERNAL_AUTH_SECRET_FILE is required.');
  }
  const resolved = pathModule.resolve(configuredPath);
  const stat = fsModule.lstatSync(resolved);
  if (stat.isSymbolicLink() || !stat.isFile()) {
    throw new Error('Stripe internal auth secret must be a regular non-symlink file.');
  }
  if (platform !== 'win32' &&
      typeof stat.mode === 'number' &&
      (stat.mode & 0o077) !== 0) {
    throw new Error('Stripe internal auth secret file must not be group/world accessible.');
  }
  const secret = String(fsModule.readFileSync(resolved, 'utf8')).trim();
  if (secret.length < 32 || secret.length > 256 || /[\u0000-\u001f\u007f]/.test(secret)) {
    throw new Error('Stripe internal auth secret must contain 32-256 printable bytes.');
  }
  return secret;
};

export const resolvePrivateInternalAuthSecretPath = ({
  pathModule,
  flagPath,
  envPath,
  peerStoresDirectory,
}) => {
  const configured = [flagPath, envPath]
    .map((value) => String(value ?? '').trim())
    .find((value) => value.length > 0);
  if (configured) return configured;

  const storesDirectory = String(peerStoresDirectory ?? '').trim();
  if (!storesDirectory) return '';
  return pathModule.resolve(
    storesDirectory,
    '..',
    'paygate',
    'internal-auth.secret'
  );
};

const safeBooleanCall = (target, method) => {
  try {
    return typeof target?.[method] === 'function' ? target[method]() === true : null;
  } catch {
    return null;
  }
};

export const adminWriterDiagnostics = (peer) => {
  const base = peer?.base;
  const writer = base?.localWriter;
  const writerCore = writer?.core;
  const applyState = base?._applyState;
  const system = applyState?.system;
  const applyStage = peer?.contract?.instance?._mayhemApplyStage;
  const viewCore = base?.view?.core;
  const applyViewCore = applyState?.view?.core;
  return {
    contract: {
      apply_stage: typeof applyStage === 'string' ? applyStage : null,
    },
    base: {
      writable: base?.writable === true,
      is_indexer: base?.isIndexer === true,
      opened: base?.opened === true,
      closing: base?.closing === true,
      paused: base?.paused === true,
      caught_up: base?._caughtup === true,
      draining: base?._draining === true,
      advancing: base?._advancing != null,
      appending_count: Array.isArray(base?._appending) ? base._appending.length : 0,
      appended_count: Number(base?._appended ?? 0),
      signed_length: Number(base?.signedLength ?? 0),
      length: Number(base?.length ?? 0),
      local_length: Number(base?.local?.length ?? 0),
      needs_wakeup: base?._needsWakeup === true,
      wakeup_hint_count: Number(base?._wakeupHints?.size ?? 0),
    },
    view: viewCore ? {
      writable: viewCore.writable === true,
      opened: viewCore.opened === true,
      closing: viewCore.closing === true,
      length: Number(viewCore.length ?? 0),
      contiguous_length: Number(viewCore.contiguousLength ?? 0),
      fork: Number(viewCore.fork ?? 0),
      signed_length: Number(viewCore.signedLength ?? 0),
      upgrading: viewCore?.core?.upgrading === true,
    } : null,
    local_writer: writer ? {
      removed: writer.isRemoved === true,
      active_indexer: writer.isActiveIndexer === true,
      idle: safeBooleanCall(writer, 'idle'),
      flushed: safeBooleanCall(writer, 'flushed'),
      length: Number(writer.length ?? 0),
      available: Number(writer.available ?? 0),
      seen_length: Number(writer.seenLength ?? 0),
      core_length: Number(writerCore?.length ?? 0),
      core_writable: writerCore?.writable === true,
      core_opened: writerCore?.opened === true,
      core_upgrading: writerCore?.core?.upgrading === true,
    } : null,
    apply_state: applyState ? {
      opened: applyState.opened === true,
      applying: applyState.applying === true,
      local_indexer: safeBooleanCall(applyState, 'isLocalIndexer'),
      local_pending_indexer: safeBooleanCall(applyState, 'isLocalPendingIndexer'),
      indexed_length: Number(applyState.indexedLength ?? 0),
      indexer_count: Array.isArray(system?.indexers) ? system.indexers.length : 0,
      pending_indexer_count: Array.isArray(system?.pendingIndexers)
        ? system.pendingIndexers.length
        : 0,
      indexer_lengths: Array.isArray(system?.indexers)
        ? system.indexers.map((entry) => Number(entry?.length ?? 0))
        : [],
      indexers_updated: system?.indexerUpdate === true,
      view: applyViewCore ? {
        writable: applyViewCore.writable === true,
        opened: applyViewCore.opened === true,
        closing: applyViewCore.closing === true,
        length: Number(applyViewCore.length ?? 0),
        contiguous_length: Number(applyViewCore.contiguousLength ?? 0),
        fork: Number(applyViewCore.fork ?? 0),
        signed_length: Number(applyViewCore.signedLength ?? 0),
        upgrading: applyViewCore?.core?.upgrading === true,
      } : null,
      views: Array.isArray(applyState.views)
        ? applyState.views.map((entry) => ({
          name: typeof entry?.name === 'string' ? entry.name : null,
          mapped_index: Number(entry?.mappedIndex ?? -1),
          length: Number(entry?.length ?? 0),
          core_writable: entry?.core?.writable === true,
          core_opened: entry?.core?.opened === true,
          core_length: Number(entry?.core?.length ?? 0),
          core_contiguous_length: Number(entry?.core?.contiguousLength ?? 0),
          core_signed_length: Number(entry?.core?.signedLength ?? 0),
          core_upgrading: entry?.core?.core?.upgrading === true,
        }))
        : [],
    } : null,
  };
};

export async function submitMayhemFeature(peer, body) {
  if (!isObject(body)) throw new Error('Missing JSON body.');
  const feature = String(body.feature ?? 'mayhem').trim();
  const key = String(body.key ?? '').trim();
  if (!feature) throw new Error('Missing feature.');
  if (!key) throw new Error('Missing key.');
  if (key.length > 256) throw new Error('Invalid key. Expected at most 256 characters.');
  if (!isObject(body.value)) throw new Error('Invalid value. Expected an object.');

  const registered = peer.protocol?.instance?.features?.[feature];
  if (!registered) throw new Error(`Invalid feature ${feature}.`);

  const admin = await peer.base?.view?.get('admin');
  const adminKey = normalizeKey(admin?.value);
  const selfKey = normalizeKey(peer.wallet?.publicKey);
  const isAdminWriter = peer.base?.writable === true && (!adminKey || adminKey === selfKey);
  if (isAdminWriter) {
    if (typeof registered.submit !== 'function') {
      throw new Error(`Invalid feature ${feature}.`);
    }
    const watchdog = setTimeout(() => {
      console.error(
        'Mayhem admin feature append stalled:',
        JSON.stringify(adminWriterDiagnostics(peer))
      );
    }, 5_000);
    try {
      return await registered.submit(key, body.value);
    } finally {
      clearTimeout(watchdog);
    }
  }
  if (feature !== 'mayhem' || typeof registered.relay !== 'function') {
    throw new Error('Peer subnet is not writable.');
  }
  return await registered.relay(key, body.value);
}

export async function requestStripeCheckout(peer, body) {
  if (!isObject(body)) throw new Error('Missing JSON body.');
  if (!isObject(body.payload) || typeof body.payload.who !== 'string' || !body.payload.who.trim()) {
    throw new Error('Missing who.');
  }
  const registered = peer.protocol?.instance?.features?.mayhem;
  if (!registered || typeof registered.requestService !== 'function') {
    throw new Error('Mayhem service relay is not ready.');
  }
  return await registered.requestService('stripe_checkout', body);
}

export async function requestStripeConnect(peer, service, body) {
  if (!isObject(body)) throw new Error('Missing JSON body.');
  if (!isObject(body.payload) ||
      typeof body.payload.provider !== 'string' || !body.payload.provider.trim()) {
    throw new Error('Missing provider.');
  }
  if (![
    'stripe_connect_onboard',
    'stripe_connect_status',
    'stripe_connect_relink',
  ].includes(service)) {
    throw new Error('Invalid Stripe Connect service.');
  }
  const registered = peer.protocol?.instance?.features?.mayhem;
  if (!registered || typeof registered.requestService !== 'function') {
    throw new Error('Mayhem service relay is not ready.');
  }
  return await registered.requestService(service, body);
}

export async function requestProviderPayoutContext(peer, body) {
  if (!isObject(body)) throw new Error('Missing JSON body.');
  if (!isObject(body.payload) ||
      typeof body.payload.provider !== 'string' ||
      !body.payload.provider.trim()) {
    throw new Error('Missing provider.');
  }
  const registered = peer.protocol?.instance?.features?.mayhem;
  if (!registered || typeof registered.requestService !== 'function') {
    throw new Error('Mayhem service relay is not ready.');
  }
  return await registered.requestService('provider_payout_context', body);
}

const errorResponse = (error) => {
  if (error?.code === 'BODY_TOO_LARGE') return [413, error.message];
  if (error?.code === 'BAD_JSON') return [400, error.message];
  const message = String(error?.message || '');
  if (/^(Missing|Invalid|Empty|Peer subnet is not writable)/.test(message)) return [400, message];
  if (message.startsWith('Mayhem feature relay') || message.startsWith('Mayhem service relay')) {
    return [503, message];
  }
  return [500, 'An internal error occurred.'];
};

export const createServer = (
  peer,
  {
    maxBodyBytes = DEFAULT_MAX_BODY_BYTES,
    allowOrigin = '*',
    releaseIdentity,
    statusMetadata = {},
  } = {}
) => {
  const sortedRoutes = [...routes].sort((a, b) => b.path.length - a.path.length);
  return http.createServer({}, async (req, res) => {
    const respond = (code, payload) => {
      if (res.headersSent) return;
      res.writeHead(code, { 'Content-Type': 'application/json' });
      res.end(JSON.stringify(payload ?? {}));
    };

    req.on('error', (error) => {
      console.error('RPC request stream error:', error);
      respond(500, { error: 'Request stream error.' });
    });
    if (applyCors(req, res, { allowOrigin })) return;

    const requestPath = (req.url || '/').split('?')[0];
    try {
      if (req.method === 'GET' && requestPath === '/v1/health') {
        if (!Number.isSafeInteger(releaseIdentity?.contractVersion) ||
            !/^[0-9a-f]{64}$/.test(releaseIdentity?.contractCodeSha256 ?? '')) {
          throw new Error('Intercom release identity is unavailable.');
        }
        return respond(200, {
          ok: true,
          contract_version: releaseIdentity.contractVersion,
          contract_code_sha256: releaseIdentity.contractCodeSha256,
        });
      }
      if (req.method === 'GET' && requestPath === '/v1/status') {
        return respond(200, await getMayhemStatus(peer, statusMetadata));
      }
      if (req.method === 'GET' && requestPath === '/v1/state') {
        const url = new URL(req.url || '/', 'http://127.0.0.1');
        if (url.searchParams.has('prefix')) {
          const confirmed = url.searchParams.get('confirmed');
          const confirmedBool = confirmed == null ? true : confirmed === 'true';
          const limit = url.searchParams.get('limit') ?? 500;
          return respond(
            200,
            await getStatePrefix(peer, url.searchParams.get('prefix'), {
              confirmed: confirmedBool,
              limit,
            })
          );
        }
      }
      if (req.method === 'POST' && requestPath === '/v1/contract/feature') {
        const body = await readJsonBody(req, { maxBytes: maxBodyBytes });
        return respond(200, await submitMayhemFeature(peer, body));
      }
      if (req.method === 'POST' && requestPath === '/v1/provider/payout/context') {
        const body = await readJsonBody(req, { maxBytes: maxBodyBytes });
        return respond(200, await requestProviderPayoutContext(peer, body));
      }
      if (req.method === 'POST' && requestPath === '/v1/payment/stripe/checkout') {
        const body = await readJsonBody(req, { maxBytes: maxBodyBytes });
        return respond(200, await requestStripeCheckout(peer, body));
      }
      if (req.method === 'POST' && requestPath === '/v1/payment/stripe/connect/onboard') {
        const body = await readJsonBody(req, { maxBytes: maxBodyBytes });
        return respond(200, await requestStripeConnect(peer, 'stripe_connect_onboard', body));
      }
      if (req.method === 'POST' && requestPath === '/v1/payment/stripe/connect/status') {
        const body = await readJsonBody(req, { maxBytes: maxBodyBytes });
        return respond(200, await requestStripeConnect(peer, 'stripe_connect_status', body));
      }
      if (req.method === 'POST' && requestPath === '/v1/payment/stripe/connect/relink') {
        const body = await readJsonBody(req, { maxBytes: maxBodyBytes });
        return respond(200, await requestStripeConnect(peer, 'stripe_connect_relink', body));
      }
      for (const route of sortedRoutes) {
        if (req.method !== route.method || requestPath !== route.path) continue;
        await route.handler({ req, res, respond, peer, maxBodyBytes });
        return;
      }
      respond(404, { error: 'Not Found' });
    } catch (error) {
      const [code, message] = errorResponse(error);
      if (code === 500) console.error('RPC handler error:', error);
      respond(code, { error: message });
    }
  });
};
