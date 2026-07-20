/** @typedef {import('pear-interface')} */
import fs from 'fs';
import http from 'http';
import path from 'path';
import crypto from 'crypto';
import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import { Peer } from 'trac-peer';
import {
  createServer as createRpcServer,
  loadPrivateInternalAuthSecret,
  resolvePrivateInternalAuthSecretPath,
} from './rpc.js';
import { hydrateAdminWriterViews, joinCanonicalPeers } from './admin-view-hydration.js';
import { installFatalRuntimeErrorPolicy } from './runtime-errors.js';
import { verifyStartupReleaseIdentity } from './release-identity.js';
import { MainSettlementBus } from 'trac-msb/src/index.js';
import { ensureTextCodecs } from 'trac-peer/src/textCodec.js';
import { getPearRuntime, ensureTrailingSlash } from 'trac-peer/src/runnerArgs.js';
import { Terminal } from 'trac-peer/src/terminal/index.js';
import {
  createMayhemMsbConfig,
  createMayhemPeerConfig,
  MAYHEM_NETWORK_ENV,
} from './network-config.js';
import Sidechannel from '../features/sidechannel/index.js';
import DirectSession from '../features/direct-session/index.js';
import InferenceRelay from '../features/inference-relay/index.js';
import ScBridge from '../features/sc-bridge/index.js';
import { resolveScBridgeToken } from '../features/sc-bridge/token.js';

const fatalRuntimeError = installFatalRuntimeErrorPolicy(
  typeof Bare !== 'undefined' ? Bare : null
);

const { argv, env, storeLabel, flags } = getPearRuntime();

const releaseIdentity = verifyStartupReleaseIdentity();
const [
  { default: MayhemProtocol },
  {
    default: MayhemContract,
    stripePayoutProcessorRevision,
    stripePayoutVerificationFeatureKey,
  },
  {
    default: MayhemFeature,
    MAYHEM_RELAY_CHANNEL,
    MAYHEM_RELAY_MAX_MESSAGE_BYTES,
  },
] = await Promise.all([
  import('../contract/protocol.js'),
  import('../contract/contract.js'),
  import('../features/mayhem/index.js'),
]);

const DEFAULT_STRIPE_WORKER_URL = 'http://127.0.0.1:11436';

const stripeWorkerEndpoint = (raw, endpointPath) => {
  const configured = String(raw ?? '').trim();
  const parsed = new URL(configured || DEFAULT_STRIPE_WORKER_URL);
  const loopback = parsed.hostname === '127.0.0.1' ||
    parsed.hostname === 'localhost' ||
    parsed.hostname === '[::1]';
  if (parsed.protocol !== 'http:' || !loopback || parsed.username || parsed.password) {
    throw new Error('MAYHEM_STRIPE_WORKER_URL must be a credential-free loopback HTTP URL.');
  }
  parsed.pathname = `${parsed.pathname.replace(/\/$/, '')}/${String(endpointPath).replace(/^\//, '')}`;
  parsed.search = '';
  parsed.hash = '';
  return parsed;
};

const stripePayoutTargetPointerKey = (provider, accountId) =>
  `payout/stripe-verified/target/${provider}/${accountId}`;

const stripeInternalAuthSecret = () => {
  return loadPrivateInternalAuthSecret({
    fsModule: fs,
    pathModule: path,
    secretPath: resolvePrivateInternalAuthSecretPath({
      pathModule: path,
      flagPath: flagValue('paygate-internal-auth-secret-file'),
      envPath: env.MAYHEM_PAYGATE_INTERNAL_AUTH_SECRET_FILE,
      peerStoresDirectory,
    }),
  });
};

const internalStripeAuthHeaders = (endpoint, body) => {
  const timestamp = String(Math.floor(Date.now() / 1_000));
  const nonce = crypto.randomBytes(32).toString('hex');
  const bodyDigest = crypto.createHash('sha256').update(body).digest('hex');
  const message = [
    'mayhem-paygate-internal-request-v1',
    timestamp,
    nonce,
    'POST',
    endpoint.pathname,
    bodyDigest,
  ].join('\n');
  const signature = crypto
    .createHmac('sha256', stripeInternalAuthSecret())
    .update(message)
    .digest('hex');
  return {
    'x-mayhem-paygate-timestamp': timestamp,
    'x-mayhem-paygate-nonce': nonce,
    'x-mayhem-paygate-signature': signature,
  };
};

const postInternalStripeRequest = (
  endpoint,
  value,
  timeoutMs = 120_000
) => new Promise((resolve, reject) => {
  const body = JSON.stringify(value);
  const authHeaders = internalStripeAuthHeaders(endpoint, body);
  const request = http.request(endpoint, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'content-length': String(b4a.byteLength(body, 'utf8')),
      ...authHeaders,
    },
  }, (response) => {
    const chunks = [];
    let bytes = 0;
    response.on('data', (chunk) => {
      bytes += chunk.length;
      if (bytes > 1_000_000) {
        request.destroy(new Error('Stripe worker response exceeded 1000000 bytes.'));
        return;
      }
      chunks.push(chunk);
    });
    response.on('end', () => {
      try {
        const text = b4a.toString(b4a.concat(chunks), 'utf8');
        const parsed = JSON.parse(text);
        if ((response.statusCode ?? 500) < 200 || (response.statusCode ?? 500) >= 300) {
          reject(new Error(parsed?.error || `Stripe worker returned HTTP ${response.statusCode}.`));
          return;
        }
        resolve(parsed);
      } catch (error) {
        reject(new Error(`Stripe worker returned an invalid response: ${error?.message ?? error}`));
      }
    });
    response.on('error', reject);
  });
  request.setTimeout(timeoutMs, () => {
    request.destroy(new Error(`Stripe worker request timed out after ${timeoutMs}ms.`));
  });
  request.on('error', reject);
  request.end(body);
});

const stripeCheckoutRequest = async (peer, value) => {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Invalid Stripe checkout request.');
  }
  const allowedKeys = new Set([
    'who',
    'au',
    'currency',
    'locale',
    'idempotency_key',
    'request_nonce',
    'success_url',
    'cancel_url',
  ]);
  const unknownKeys = Object.keys(value).filter((key) => !allowedKeys.has(key));
  if (unknownKeys.length > 0) {
    throw new Error(`Stripe checkout request does not accept fields: ${unknownKeys.join(', ')}.`);
  }
  if (!/^[0-9a-f]{64}$/.test(String(value.who || '').toLowerCase())) {
    throw new Error('Invalid Stripe checkout participant.');
  }
  if (!/^[1-9][0-9]*$/.test(String(value.au || ''))) {
    throw new Error('Invalid Stripe checkout amount.');
  }
  for (const field of ['success_url', 'cancel_url']) {
    let parsed;
    try {
      parsed = new URL(String(value[field] || ''));
    } catch {
      throw new Error(`Invalid Stripe checkout ${field}.`);
    }
    if (parsed.protocol !== 'https:' || parsed.username || parsed.password) {
      throw new Error(`Stripe checkout ${field} must be an unauthenticated HTTPS URL.`);
    }
  }
  if (value.idempotency_key !== undefined &&
      (typeof value.idempotency_key !== 'string' || value.idempotency_key.length > 255)) {
    throw new Error('Invalid Stripe checkout idempotency key.');
  }
  const requestNonce = String(value.request_nonce || '').toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(requestNonce)) {
    throw new Error('Invalid Stripe checkout request nonce.');
  }

  const payments = (await peer.base.view.get('payments/current'))?.value;
  if (!payments || payments.denom !== 'au_usd' || payments.set_by_role !== 'admin') {
    throw new Error('Canonical payment configuration is unavailable.');
  }
  const currency = String(value.currency || '').toLowerCase();
  const locale = String(value.locale || '');
  if (payments.fiat?.processor !== 'stripe' ||
      !Array.isArray(payments.fiat.currencies) ||
      !payments.fiat.currencies.includes(currency)) {
    throw new Error('Stripe checkout currency is not enabled by the admin.');
  }
  if (payments.fiat.locale !== locale) {
    throw new Error('Stripe checkout locale is not enabled by the admin.');
  }

  const endpoint = stripeWorkerEndpoint(
    stripeWorkerUrl,
    '/v1/stripe/checkout-sessions'
  );
  const workerValue = { ...value };
  delete workerValue.request_nonce;
  if (!workerValue.idempotency_key) {
    workerValue.idempotency_key = `mayhem-checkout-${requestNonce}`;
  }
  return await postInternalStripeRequest(endpoint, {
    ...workerValue,
    currency,
    locale,
  }, stripeWorkerRequestTimeoutMs);
};

const validateStripeConnectRequest = async (peer, service, value) => {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Invalid Stripe Connect request.');
  }
  const allowedKeys = service === 'stripe_connect_onboard'
    ? new Set([
        'provider',
        'country',
        'request_nonce',
        'rotate',
        'previous_account_id',
      ])
    : service === 'stripe_connect_relink'
      ? new Set([
          'provider',
          'source_provider',
          'account_id',
          'context_revision',
          'country',
          'request_nonce',
          'consent_expires_at',
          'source_consent_signature',
        ])
      : new Set(['provider', 'request_nonce']);
  const unknownKeys = Object.keys(value).filter((key) => !allowedKeys.has(key));
  if (unknownKeys.length > 0) {
    throw new Error(`Stripe Connect request does not accept fields: ${unknownKeys.join(', ')}.`);
  }
  const rawProvider = String(value.provider || '');
  const provider = rawProvider.toLowerCase();
  if (rawProvider !== provider || !/^[0-9a-f]{64}$/.test(provider)) {
    throw new Error('Invalid Stripe Connect provider.');
  }
  const requestNonce = String(value.request_nonce || '');
  if (requestNonce !== requestNonce.toLowerCase() ||
      !/^[0-9a-f]{64}$/.test(requestNonce)) {
    throw new Error('Invalid Stripe Connect request nonce.');
  }
  const providerRecord = (await peer.base.view.get(`prov/${provider}`))?.value;
  if (!providerRecord || providerRecord.status !== 'active') {
    throw new Error('Stripe Connect onboarding requires an active provider.');
  }
  if (!Array.isArray(providerRecord.accepted_rails) ||
      !providerRecord.accepted_rails.includes('fiat')) {
    throw new Error('Stripe Connect onboarding requires the provider to accept fiat.');
  }
  if (service === 'stripe_connect_onboard' || service === 'stripe_connect_relink') {
    const country = String(value.country || '').toUpperCase();
    if (!/^[A-Z]{2}$/.test(country)) {
      throw new Error('Stripe Connect country must be a two-letter ISO country code.');
    }
    if (service === 'stripe_connect_onboard') {
      const rotate = value.rotate === true;
      const previousAccountId = value.previous_account_id == null
        ? null
        : String(value.previous_account_id);
      if ((value.rotate !== undefined && typeof value.rotate !== 'boolean') ||
          (rotate && !/^acct_[A-Za-z0-9._-]+$/.test(previousAccountId || '')) ||
          (!rotate && previousAccountId !== null)) {
        throw new Error('Stripe Connect rotation is not canonical.');
      }
      return {
        ...value,
        provider,
        country,
        rotate,
        previous_account_id: previousAccountId,
      };
    }
    if (service === 'stripe_connect_relink') {
      const rawSourceProvider = String(value.source_provider || '');
      const sourceProvider = rawSourceProvider.toLowerCase();
      if (rawSourceProvider !== sourceProvider ||
          !/^[0-9a-f]{64}$/.test(sourceProvider) ||
          sourceProvider === provider) {
        throw new Error('Stripe Connect relink requires a different source provider identity.');
      }
      const accountId = String(value.account_id || '');
      const contextRevision = String(value.context_revision || '');
      const sourceConsentSignature = String(value.source_consent_signature || '');
      if (!/^acct_[A-Za-z0-9._-]+$/.test(accountId) ||
          !/^[0-9a-f]{64}$/.test(contextRevision) ||
          !/^[0-9a-f]{128}$/.test(sourceConsentSignature) ||
          !Number.isSafeInteger(value.consent_expires_at)) {
        throw new Error('Stripe Connect relink consent is not canonical.');
      }
      const now = Math.floor(Date.now() / 1_000);
      if (value.consent_expires_at <= now ||
          value.consent_expires_at > now + 600) {
        throw new Error('Stripe Connect relink consent is stale or exceeds its bounded lifetime.');
      }
      const sourceProviderRecord = (
        await peer.base.view.get(`prov/${sourceProvider}`)
      )?.value;
      if (!sourceProviderRecord ||
          sourceProviderRecord.status !== 'active' ||
          !Array.isArray(sourceProviderRecord.accepted_rails) ||
          !sourceProviderRecord.accepted_rails.includes('fiat')) {
        throw new Error('Stripe Connect relink source must be an active fiat provider.');
      }
      const contextPointer = (await peer.base.view.get('payout/context/current'))?.value;
      const context = contextPointer?.record_key
        ? (await peer.base.view.get(contextPointer.record_key))?.value
        : null;
      if (!context ||
          context.type !== 'provider_payout_context' ||
          context.revision !== contextRevision ||
          contextPointer.revision !== contextRevision ||
          context.admin !== peer.wallet.publicKey ||
          context.published_by !== peer.wallet.publicKey ||
          context.published_by_role !== 'admin') {
        throw new Error('Stripe Connect relink consent does not bind the current payout context.');
      }
      const sourceVerificationPointer = (
        await peer.base.view.get(stripePayoutTargetPointerKey(sourceProvider, accountId))
      )?.value;
      const sourceVerification = sourceVerificationPointer?.record_key
        ? (await peer.base.view.get(sourceVerificationPointer.record_key))?.value
        : null;
      if (!sourceVerification ||
          sourceVerification.type !== 'stripe_payout_verification' ||
          sourceVerification.provider !== sourceProvider ||
          sourceVerification.target !== accountId ||
          sourceVerification.context_revision !== contextRevision ||
          sourceVerification.ready !== true ||
          sourceVerification.details_submitted !== true ||
          sourceVerification.payouts_enabled !== true ||
          sourceVerification.transfers_enabled !== true ||
          sourceVerification.verified_by !== peer.wallet.publicKey ||
          sourceVerification.verified_by_role !== 'admin') {
        throw new Error('Stripe Connect relink source account is not currently verified and ready.');
      }
      return {
        ...value,
        provider,
        source_provider: sourceProvider,
        account_id: accountId,
        context_revision: contextRevision,
        country,
        source_consent_signature: sourceConsentSignature,
      };
    }
    return { ...value, provider, country };
  }
  return { ...value, provider };
};

const waitForStripePayoutVerification = async (
  peer,
  provider,
  accountId,
  recordKey
) => {
  for (let attempt = 0; attempt < 120; attempt += 1) {
    const pointer = (
      await peer.base.view.get(stripePayoutTargetPointerKey(provider, accountId))
    )?.value;
    const record = pointer?.record_key === recordKey
      ? (await peer.base.view.get(recordKey))?.value
      : null;
    if (record?.type === 'stripe_payout_verification' &&
        record.provider === provider &&
        record.verified_by === peer.wallet.publicKey &&
        record.verified_by_role === 'admin') {
      return record;
    }
    await new Promise((resolve) => setTimeout(resolve, 500));
  }
  throw new Error('Timed out waiting for provider-scoped Stripe payout verification.');
};

const verifyStripePayoutTarget = async (
  peer,
  provider,
  account,
  mode,
  requestNonce,
  verificationKind,
  sourceProvider,
  previousVerification
) => {
  const accountId = String(account?.id || '');
  const currency = String(account?.default_currency || '').toLowerCase();
  if (!/^acct_[A-Za-z0-9._-]+$/.test(accountId)) {
    throw new Error('Stripe Connect status returned an invalid account id.');
  }
  const payments = (await peer.base.view.get('payments/current'))?.value;
  if (!Array.isArray(payments?.fiat?.currencies) ||
      !payments.fiat.currencies.includes(currency)) {
    throw new Error(`Stripe payout currency ${currency || '(missing)'} is not enabled by the admin.`);
  }
  if (!['express', 'standard', 'custom'].includes(account?.account_type) ||
      typeof account?.details_submitted !== 'boolean' ||
      typeof account?.payouts_enabled !== 'boolean' ||
      typeof account?.transfers_enabled !== 'boolean' ||
      account.ready !== (
        account.details_submitted &&
        account.payouts_enabled &&
        account.transfers_enabled
      )) {
    throw new Error('Stripe Connect account returned invalid payout readiness.');
  }
  if (!['live', 'test'].includes(mode)) {
    throw new Error('Stripe Connect worker returned an invalid mode.');
  }
  const contextPointer = (await peer.base.view.get('payout/context/current'))?.value;
  const context = contextPointer?.record_key
    ? (await peer.base.view.get(contextPointer.record_key))?.value
    : null;
  if (!context ||
      context.type !== 'provider_payout_context' ||
      context.revision !== contextPointer.revision ||
      context.payment_config_version !== payments?.ver ||
      context.admin !== peer.wallet.publicKey ||
      context.published_by !== peer.wallet.publicKey ||
      context.published_by_role !== 'admin') {
    throw new Error('Current admin-published payout context is unavailable.');
  }
  const value = {
    op: 'verify_stripe_payout',
    provider,
    account_id: accountId,
    account_type: account.account_type,
    country: account.country,
    currency,
    mode,
    verification_kind: verificationKind,
    source_provider: sourceProvider,
    processor_revision: null,
    previous_verification: previousVerification,
    details_submitted: account.details_submitted,
    payouts_enabled: account.payouts_enabled,
    transfers_enabled: account.transfers_enabled,
    network: context.network,
    admin: context.admin,
    bootstrap: context.bootstrap,
    context_revision: context.revision,
    payment_config_version: context.payment_config_version,
    request_nonce: String(requestNonce).toLowerCase(),
  };
  value.processor_revision = await stripePayoutProcessorRevision(value);
  const key = await stripePayoutVerificationFeatureKey(value);
  const currentPointer = (
    await peer.base.view.get(stripePayoutTargetPointerKey(provider, accountId))
  )?.value;
  const currentRecord = currentPointer?.record_key
    ? (await peer.base.view.get(currentPointer.record_key))?.value
    : null;
  if (currentRecord?.target === accountId &&
      currentRecord.currency === currency &&
      currentRecord.context_revision === context.revision &&
      currentRecord.payment_config_version === context.payment_config_version &&
      currentRecord.details_submitted === account.details_submitted &&
      currentRecord.payouts_enabled === account.payouts_enabled &&
      currentRecord.transfers_enabled === account.transfers_enabled) {
    return { ok: true, changed: false, verification: currentRecord };
  }
  await peer.mayhemFeature.record(key, value);
  return {
    ok: true,
    changed: true,
    verification: await waitForStripePayoutVerification(
      peer,
      provider,
      accountId,
      key
    ),
  };
};

const stripeConnectService = async (peer, service, value, authorization) => {
  const request = await validateStripeConnectRequest(peer, service, value);
  const verificationPointer = (
    await peer.base.view.get(`payout/stripe-verified/current/${request.provider}`)
  )?.value;
  const previousVerification = verificationPointer?.revision ?? null;
  const workerPath = service === 'stripe_connect_onboard'
    ? '/v1/stripe/connect/onboard'
    : service === 'stripe_connect_relink'
      ? '/v1/stripe/connect/relink'
      : '/v1/stripe/connect/status';
  const endpoint = stripeWorkerEndpoint(stripeWorkerUrl, workerPath);
  const workerRequest = service === 'stripe_connect_relink'
    ? {
        ...request,
        target_service_signature: authorization?.signature,
      }
    : request;
  if (service === 'stripe_connect_relink' &&
      (authorization?.actor !== request.provider ||
        !/^[0-9a-f]{128}$/.test(String(authorization?.signature || '')))) {
    throw new Error('Stripe Connect relink target authorization is unavailable.');
  }
  const response = await postInternalStripeRequest(
    endpoint,
    workerRequest,
    stripeWorkerRequestTimeoutMs
  );
  if (response?.ok !== true || response?.rail !== 'fiat' ||
      response?.processor_rail !== 'stripe' || response?.provider !== request.provider) {
    throw new Error('Stripe Connect worker returned an invalid response.');
  }
  if (service === 'stripe_connect_relink' &&
      response.source_provider !== request.source_provider) {
    throw new Error('Stripe Connect relink worker returned the wrong source provider.');
  }
  const payoutVerification = response.account
    ? await verifyStripePayoutTarget(
        peer,
        request.provider,
        response.account,
        response.mode,
        request.request_nonce,
        service.slice('stripe_connect_'.length),
        service === 'stripe_connect_relink' ? request.source_provider : null,
        previousVerification
      )
    : { ok: true, changed: false, pending: true };
  return { ...response, payout_verification: payoutVerification };
};

const stripeAdminService = (peer) => {
  const payoutBindings = new Map();
  return async (service, value, authorization) => {
    if (service === 'stripe_checkout') return await stripeCheckoutRequest(peer, value);
    if (![
      'stripe_connect_onboard',
      'stripe_connect_status',
      'stripe_connect_relink',
    ].includes(service)) {
      throw new Error('Unsupported Mayhem admin service.');
    }
    const provider = String(value?.provider || '').toLowerCase();
    const previous = payoutBindings.get(provider) || Promise.resolve();
    const current = previous
      .catch(() => {})
      .then(() => stripeConnectService(peer, service, value, authorization));
    payoutBindings.set(provider, current);
    try {
      return await current;
    } finally {
      if (payoutBindings.get(provider) === current) payoutBindings.delete(provider);
    }
  };
};

if (flags['msb-transfer-helper']) {
  try {
    const marker = argv.findIndex((arg) =>
      String(arg).startsWith('--msb-transfer-helper')
    );
    const rawMarker = marker >= 0 ? String(argv[marker]) : '';
    const command = rawMarker.includes('=')
      ? rawMarker.slice(rawMarker.indexOf('=') + 1)
      : String(argv[marker + 1] || '');
    const helperArgs = argv.slice(marker + (rawMarker.includes('=') ? 1 : 2));
    const { runRootMsbTransferHelper } = await import('./msb-transfer-helper.js');
    const output = await runRootMsbTransferHelper(command, helperArgs);
    console.log(JSON.stringify(output));
    Bare.exit(0);
  } catch (error) {
    console.error(error?.message ?? error);
    Bare.exit(1);
  }
}

if (flags['wallet-helper']) {
  try {
    const {
      readWalletHelperSecretsFromStdin,
      runWalletHelper,
    } = await import('./wallet-helper.js');
    const secrets = await readWalletHelperSecretsFromStdin();
    const output = await runWalletHelper(flags, secrets);
    console.log(JSON.stringify(output));
    Bare.exit(0);
  } catch (error) {
    console.error(error?.message ?? error);
    Bare.exit(1);
  }
}

class MayhemWallet extends PeerWallet {
  get publicKey() {
    const publicKey = super.publicKey;
    return publicKey ? b4a.toString(publicKey, 'hex') : null;
  }

  get secretKey() {
    const secretKey = super.secretKey;
    return secretKey ? b4a.toString(secretKey, 'hex') : null;
  }

  sign(message) {
    const messageBuffer = b4a.isBuffer(message) ? message : b4a.from(String(message));
    const signature = super.sign(messageBuffer);
    return b4a.toString(signature, 'hex');
  }

  verify(signature, message, publicKey = this.publicKey) {
    const signatureBuffer = b4a.isBuffer(signature) ? signature : b4a.from(String(signature), 'hex');
    const messageBuffer = b4a.isBuffer(message) ? message : b4a.from(String(message));
    const publicKeyBuffer = b4a.isBuffer(publicKey) ? publicKey : b4a.from(String(publicKey), 'hex');
    return PeerWallet.verify(signatureBuffer, messageBuffer, publicKeyBuffer);
  }
}

const parseBool = (value, fallback) => {
  if (value === undefined || value === null || value === '') return fallback;
  return ['1', 'true', 'yes', 'on'].includes(String(value).trim().toLowerCase());
};

const flagValue = (name, fallback = undefined) => {
  const value = flags[name];
  if (value === undefined) return fallback;
  if (value === true) return 'true';
  return String(value);
};

const parseInteger = (value, fallback) => {
  if (value === undefined || value === null || value === '') return fallback;
  const parsed = Number.parseInt(String(value), 10);
  return Number.isSafeInteger(parsed) ? parsed : fallback;
};

const parseCsvList = (raw) => {
  if (!raw) return null;
  return String(raw)
    .split(',')
    .map((value) => value.trim())
    .filter((value) => value.length > 0);
};

const normalizeNetworkEnv = (raw) => {
  const value = String(raw || 'mainnet').trim().toLowerCase();
  if (value === 'local') return 'local';
  if (value === 'mainnet') return 'mainnet';
  if (value === 'development' || value === 'dev') return 'development';
  if (value === 'testnet' || value === 'testnet1') return 'testnet1';
  throw new Error(`Unsupported --network value "${raw}". Expected mainnet, testnet1, local, or development.`);
};

const networkEnv = normalizeNetworkEnv(
  (flags.network && String(flags.network)) ||
    (flags['network-env'] && String(flags['network-env'])) ||
    env.MAYHEM_NETWORK ||
    env.TRAC_NETWORK_ENV ||
    ''
);

const contractNetworkEnvironment = networkEnv === 'mainnet'
  ? MAYHEM_NETWORK_ENV.MAINNET
  : networkEnv === 'testnet1'
    ? MAYHEM_NETWORK_ENV.TESTNET1
    : MAYHEM_NETWORK_ENV.DEVELOPMENT;

const headless = parseBool(flagValue('headless', env.MAYHEM_HEADLESS || ''), false);
const peerInteractive = parseBool(
  flagValue('peer-interactive', env.PEER_INTERACTIVE || ''),
  !headless
);
const peerBackgroundTasks = parseBool(
  flagValue('peer-background-tasks', env.PEER_BACKGROUND_TASKS || ''),
  true
);
const peerUpdater = parseBool(
  flagValue('peer-updater', env.PEER_UPDATER || ''),
  true
);
const peerReplicate = parseBool(
  flagValue('peer-replicate', env.PEER_REPLICATE || ''),
  true
);
const peerDirectPeers = parseCsvList(
  flagValue('peer-direct-peer', env.PEER_DIRECT_PEERS || '')
) || [];
if (peerDirectPeers.length > 16 ||
    peerDirectPeers.some((key) => !/^[0-9a-fA-F]{64}$/.test(key))) {
  throw new Error('Invalid --peer-direct-peer. Expected at most 16 comma-separated 32-byte public keys.');
}
const keepAlive = parseBool(flagValue('keep-alive', env.MAYHEM_KEEP_ALIVE || ''), headless);

const peerStoreName =
  (flags['peer-store-name'] && String(flags['peer-store-name'])) ||
  env.PEER_STORE_NAME ||
  storeLabel ||
  'peer';

const peerStoresDirectory = ensureTrailingSlash(
  (flags['peer-stores-directory'] && String(flags['peer-stores-directory'])) ||
    env.PEER_STORES_DIRECTORY ||
    'stores/'
);

const msbStoreName =
  (flags['msb-store-name'] && String(flags['msb-store-name'])) ||
  env.MSB_STORE_NAME ||
  `${peerStoreName}-msb`;

const msbStoresDirectory = ensureTrailingSlash(
  (flags['msb-stores-directory'] && String(flags['msb-stores-directory'])) ||
    env.MSB_STORES_DIRECTORY ||
    'stores/'
);

const subnetChannel =
  (flags['subnet-channel'] && String(flags['subnet-channel'])) ||
  env.SUBNET_CHANNEL ||
  'mayhem-router-subnet';

const subnetBootstrapHex =
  (flags['subnet-bootstrap'] && String(flags['subnet-bootstrap'])) ||
  env.SUBNET_BOOTSTRAP ||
  null;

const sidechannelEntry = '0000intercom';
const sidechannelsRaw =
  (flags.sidechannels && String(flags.sidechannels)) ||
  (flags.sidechannel && String(flags.sidechannel)) ||
  env.SIDECHANNELS ||
  '';
const sidechannelExtras = sidechannelsRaw
  .split(',')
  .map((value) => value.trim())
  .filter((value) => value.length > 0 && value !== sidechannelEntry);

const sidechannelQuiet = parseBool(
  (flags['sidechannel-quiet'] && String(flags['sidechannel-quiet'])) || env.SIDECHANNEL_QUIET || '',
  false
);
const sidechannelDebug = parseBool(
  (flags['sidechannel-debug'] && String(flags['sidechannel-debug'])) || env.SIDECHANNEL_DEBUG || '',
  false
);
const sidechannelMaxBytes = Number.parseInt(
  (flags['sidechannel-max-bytes'] && String(flags['sidechannel-max-bytes'])) ||
    env.SIDECHANNEL_MAX_BYTES ||
    '',
  10
);
const sidechannelAllowRemoteOpen = parseBool(
  (flags['sidechannel-allow-remote-open'] && String(flags['sidechannel-allow-remote-open'])) ||
    env.SIDECHANNEL_ALLOW_REMOTE_OPEN ||
    '',
  true
);
const sidechannelAutoJoin = parseBool(
  (flags['sidechannel-auto-join'] && String(flags['sidechannel-auto-join'])) ||
    env.SIDECHANNEL_AUTO_JOIN ||
    '',
  false
);
const sidechannelWelcomeRequired = parseBool(
  (flags['sidechannel-welcome-required'] && String(flags['sidechannel-welcome-required'])) ||
    env.SIDECHANNEL_WELCOME_REQUIRED ||
    '',
  false
);
const sidechannelMuxRetryMax = parseInteger(
  flagValue('sidechannel-mux-retry-max', env.SIDECHANNEL_MUX_RETRY_MAX || ''),
  5
);
const sidechannelMuxRetryDelayMs = parseInteger(
  flagValue('sidechannel-mux-retry-delay-ms', env.SIDECHANNEL_MUX_RETRY_DELAY_MS || ''),
  50
);
const sidechannelOpenRetryMax = parseInteger(
  flagValue('sidechannel-open-retry-max', env.SIDECHANNEL_OPEN_RETRY_MAX || ''),
  5
);
const sidechannelOpenRetryBaseMs = parseInteger(
  flagValue('sidechannel-open-retry-base-ms', env.SIDECHANNEL_OPEN_RETRY_BASE_MS || ''),
  100
);
const sidechannelOpenRetryResetMs = parseInteger(
  flagValue('sidechannel-open-retry-reset-ms', env.SIDECHANNEL_OPEN_RETRY_RESET_MS || ''),
  2_000
);
const sidechannelFlushTimeoutMs = parseInteger(
  flagValue('sidechannel-flush-timeout-ms', env.SIDECHANNEL_FLUSH_TIMEOUT_MS || ''),
  10_000
);
const sidechannelDirectConnectMaxWaitMs = parseInteger(
  flagValue(
    'sidechannel-direct-connect-max-wait-ms',
    env.SIDECHANNEL_DIRECT_CONNECT_MAX_WAIT_MS || ''
  ),
  120_000
);
const sidechannelDirectConnectPollMs = parseInteger(
  flagValue(
    'sidechannel-direct-connect-poll-ms',
    env.SIDECHANNEL_DIRECT_CONNECT_POLL_MS || ''
  ),
  100
);
const sidechannelMaxChannels = parseInteger(
  flagValue('sidechannel-max-channels', env.SIDECHANNEL_MAX_CHANNELS || ''),
  1024
);
const sidechannelMaxChannelNameBytes = parseInteger(
  flagValue('sidechannel-max-channel-name-bytes', env.SIDECHANNEL_MAX_CHANNEL_NAME_BYTES || ''),
  256
);
const sidechannelChannelOpenTimeoutMs = parseInteger(
  flagValue('sidechannel-channel-open-timeout-ms', env.SIDECHANNEL_CHANNEL_OPEN_TIMEOUT_MS || ''),
  120_000
);
if (
  sidechannelMuxRetryMax < 0 ||
  sidechannelMuxRetryDelayMs <= 0 ||
  sidechannelOpenRetryMax < 0 ||
  sidechannelOpenRetryBaseMs <= 0 ||
  sidechannelOpenRetryResetMs <= 0 ||
  sidechannelFlushTimeoutMs <= 0 ||
  sidechannelDirectConnectMaxWaitMs <= 0 ||
  sidechannelDirectConnectPollMs <= 0 ||
  sidechannelMaxChannels <= 0 ||
  sidechannelMaxChannelNameBytes <= 0 ||
  sidechannelChannelOpenTimeoutMs <= 0
) {
  throw new Error('Sidechannel retry and flush timing values are invalid.');
}
const mayhemRelayPowDifficulty = parseInteger(
  flagValue('mayhem-relay-pow-difficulty', env.MAYHEM_RELAY_POW_DIFFICULTY || ''),
  18
);
const mayhemRelayMaxMessageBytes = parseInteger(
  flagValue('mayhem-relay-max-bytes', env.MAYHEM_RELAY_MAX_BYTES || ''),
  MAYHEM_RELAY_MAX_MESSAGE_BYTES
);
const mayhemRelayTimeoutMs = parseInteger(
  flagValue('mayhem-relay-timeout-ms', env.MAYHEM_RELAY_TIMEOUT_MS || ''),
  0
);
const mayhemRelayRetryMs = parseInteger(
  flagValue('mayhem-relay-retry-ms', env.MAYHEM_RELAY_RETRY_MS || ''),
  1_000
);
const mayhemRelayConnectTimeoutMs = parseInteger(
  flagValue('mayhem-relay-connect-timeout-ms', env.MAYHEM_RELAY_CONNECT_TIMEOUT_MS || ''),
  15_000
);
const mayhemRelayResultTimeoutMs = parseInteger(
  flagValue('mayhem-relay-result-timeout-ms', env.MAYHEM_RELAY_RESULT_TIMEOUT_MS || ''),
  0
);
const mayhemRelayResultPollMs = parseInteger(
  flagValue('mayhem-relay-result-poll-ms', env.MAYHEM_RELAY_RESULT_POLL_MS || ''),
  50
);
const stripeWorkerRequestTimeoutMs = parseInteger(
  flagValue(
    'stripe-worker-request-timeout-ms',
    env.MAYHEM_STRIPE_WORKER_REQUEST_TIMEOUT_MS || ''
  ),
  120_000
);
if (stripeWorkerRequestTimeoutMs <= 0) {
  throw new Error('Stripe worker request timeout must be positive.');
}
const stripeWorkerUrl = flagValue(
  'stripe-worker-url',
  env.MAYHEM_STRIPE_WORKER_URL || DEFAULT_STRIPE_WORKER_URL
);
const directSessionDebug = parseBool(
  (flags['session-debug'] && String(flags['session-debug'])) || env.SESSION_DEBUG || '',
  false
);
const directSessionMaxFrameBytes = Number.parseInt(
  (flags['session-max-frame-bytes'] && String(flags['session-max-frame-bytes'])) ||
    env.SESSION_MAX_FRAME_BYTES ||
    '',
  10
);
const directSessionRateBytesPerSecond = Number.parseInt(
  (flags['session-rate-bytes-per-second'] && String(flags['session-rate-bytes-per-second'])) ||
    env.SESSION_RATE_BYTES_PER_SECOND ||
    '',
  10
);
const directSessionRateBurstBytes = Number.parseInt(
  (flags['session-rate-burst-bytes'] && String(flags['session-rate-burst-bytes'])) ||
    env.SESSION_RATE_BURST_BYTES ||
    '',
  10
);
const directSessionSendDrainTimeoutMs = Number.parseInt(
  (flags['session-send-drain-timeout-ms'] &&
    String(flags['session-send-drain-timeout-ms'])) ||
    env.SESSION_SEND_DRAIN_TIMEOUT_MS ||
    '',
  10
);
const directSessionConnectMaxWaitMs = Number.parseInt(
  (flags['session-connect-max-wait-ms'] && String(flags['session-connect-max-wait-ms'])) ||
    env.SESSION_CONNECT_MAX_WAIT_MS ||
    '',
  10
);
const directSessionConnectPollMs = Number.parseInt(
  (flags['session-connect-poll-ms'] && String(flags['session-connect-poll-ms'])) ||
    env.SESSION_CONNECT_POLL_MS ||
    '',
  10
);
const directSessionOpenMaxWaitMs = Number.parseInt(
  (flags['session-open-max-wait-ms'] && String(flags['session-open-max-wait-ms'])) ||
    env.SESSION_OPEN_MAX_WAIT_MS ||
    '',
  10
);
const directSessionHealthIntervalMs = Number.parseInt(
  (flags['session-health-interval-ms'] && String(flags['session-health-interval-ms'])) ||
    env.SESSION_HEALTH_INTERVAL_MS ||
    '',
  10
);
const directSessionHealthFreshMs = Number.parseInt(
  (flags['session-health-fresh-ms'] && String(flags['session-health-fresh-ms'])) ||
    env.SESSION_HEALTH_FRESH_MS ||
    '',
  10
);
const directSessionHealthTimeoutMs = Number.parseInt(
  (flags['session-health-timeout-ms'] && String(flags['session-health-timeout-ms'])) ||
    env.SESSION_HEALTH_TIMEOUT_MS ||
    '',
  10
);
const directSessionMaxSessions = Number.parseInt(
  (flags['session-max-sessions'] && String(flags['session-max-sessions'])) ||
    env.SESSION_MAX_SESSIONS ||
    '',
  10
);
const directSessionMaxSessionsPerConnection = Number.parseInt(
  (flags['session-max-sessions-per-connection'] &&
    String(flags['session-max-sessions-per-connection'])) ||
    env.SESSION_MAX_SESSIONS_PER_CONNECTION ||
    '',
  10
);

const inferenceRelayServe = parseBool(
  flagValue('inference-relay-serve', env.MAYHEM_INFERENCE_RELAY_SERVE || ''),
  false
);
const inferenceRelayPeers = parseCsvList(
  flagValue('inference-relay-peers', env.MAYHEM_INFERENCE_RELAY_PEERS || '')
) || [];
const inferenceRelayForce = parseBool(
  flagValue('inference-relay-force', env.MAYHEM_INFERENCE_RELAY_FORCE || ''),
  false
);
const inferenceRelayDirectWaitMs = parseInteger(
  flagValue('inference-relay-direct-wait-ms', env.MAYHEM_INFERENCE_RELAY_DIRECT_WAIT_MS || ''),
  15_000
);
const inferenceRelayWaitMs = parseInteger(
  flagValue('inference-relay-wait-ms', env.MAYHEM_INFERENCE_RELAY_WAIT_MS || ''),
  45_000
);
const inferenceRelayMaxClients = parseInteger(
  flagValue('inference-relay-max-clients', env.MAYHEM_INFERENCE_RELAY_MAX_CLIENTS || ''),
  256
);
const inferenceRelayMaxPendingPairs = parseInteger(
  flagValue(
    'inference-relay-max-pending-pairs',
    env.MAYHEM_INFERENCE_RELAY_MAX_PENDING_PAIRS || ''
  ),
  512
);
const inferenceRelayMaxActivePairs = parseInteger(
  flagValue(
    'inference-relay-max-active-pairs',
    env.MAYHEM_INFERENCE_RELAY_MAX_ACTIVE_PAIRS || ''
  ),
  128
);
const inferenceRelayMaxBytesPerLink = parseInteger(
  flagValue(
    'inference-relay-max-bytes-per-link',
    env.MAYHEM_INFERENCE_RELAY_MAX_BYTES_PER_LINK || ''
  ),
  512 * 1024 * 1024
);
const inferenceRelayRateBytesPerSecond = parseInteger(
  flagValue(
    'inference-relay-rate-bytes-per-second',
    env.MAYHEM_INFERENCE_RELAY_RATE_BYTES_PER_SECOND || ''
  ),
  32 * 1024 * 1024
);
const inferenceRelayRateBurstBytes = parseInteger(
  flagValue(
    'inference-relay-rate-burst-bytes',
    env.MAYHEM_INFERENCE_RELAY_RATE_BURST_BYTES || ''
  ),
  64 * 1024 * 1024
);
const inferenceRelayIdleTimeoutMs = parseInteger(
  flagValue(
    'inference-relay-idle-timeout-ms',
    env.MAYHEM_INFERENCE_RELAY_IDLE_TIMEOUT_MS || ''
  ),
  120_000
);
if (
  inferenceRelayDirectWaitMs <= 0
  || inferenceRelayWaitMs <= 0
  || inferenceRelayMaxClients <= 0
  || inferenceRelayMaxPendingPairs <= 0
  || inferenceRelayMaxActivePairs <= 0
  || inferenceRelayMaxBytesPerLink <= 0
  || inferenceRelayRateBytesPerSecond <= 0
  || inferenceRelayRateBurstBytes <= 0
  || inferenceRelayIdleTimeoutMs <= 0
) {
  throw new Error('Inference relay limits and timeouts must be positive.');
}

const scBridgeEnabled = parseBool(
  (flags['sc-bridge'] && String(flags['sc-bridge'])) || env.SC_BRIDGE || '',
  false
);
const scBridgeHost =
  (flags['sc-bridge-host'] && String(flags['sc-bridge-host'])) ||
  env.SC_BRIDGE_HOST ||
  '127.0.0.1';
const scBridgePort = Number.parseInt(
  (flags['sc-bridge-port'] && String(flags['sc-bridge-port'])) || env.SC_BRIDGE_PORT || '',
  10
);
const scBridgeToken = resolveScBridgeToken(flags, env);
const scBridgeCliEnabled = parseBool(
  (flags['sc-bridge-cli'] && String(flags['sc-bridge-cli'])) || env.SC_BRIDGE_CLI || '',
  false
);
const scBridgeDebug = parseBool(
  (flags['sc-bridge-debug'] && String(flags['sc-bridge-debug'])) || env.SC_BRIDGE_DEBUG || '',
  false
);
const scBridgeMaxClients = parseInteger(
  flagValue('sc-bridge-max-clients', env.SC_BRIDGE_MAX_CLIENTS || ''),
  64
);
const scBridgeMaxMessageBytes = parseInteger(
  flagValue('sc-bridge-max-message-bytes', env.SC_BRIDGE_MAX_MESSAGE_BYTES || ''),
  2 * 1024 * 1024
);
const scBridgeMaxSubscriptionsPerClient = parseInteger(
  flagValue(
    'sc-bridge-max-subscriptions-per-client',
    env.SC_BRIDGE_MAX_SUBSCRIPTIONS_PER_CLIENT || ''
  ),
  4096
);
const scBridgeMaxOutboundMessagesPerClient = parseInteger(
  flagValue(
    'sc-bridge-max-outbound-messages-per-client',
    env.SC_BRIDGE_MAX_OUTBOUND_MESSAGES_PER_CLIENT || ''
  ),
  4096
);
const scBridgeMaxOutboundBytesPerClient = parseInteger(
  flagValue(
    'sc-bridge-max-outbound-bytes-per-client',
    env.SC_BRIDGE_MAX_OUTBOUND_BYTES_PER_CLIENT || ''
  ),
  64 * 1024 * 1024
);
const scBridgeAuthTimeoutMs = parseInteger(
  flagValue('sc-bridge-auth-timeout-ms', env.SC_BRIDGE_AUTH_TIMEOUT_MS || ''),
  10_000
);
const scBridgeMaxCliQueue = parseInteger(
  flagValue('sc-bridge-max-cli-queue', env.SC_BRIDGE_MAX_CLI_QUEUE || ''),
  64
);
if (
  scBridgeMaxClients <= 0 ||
  scBridgeMaxMessageBytes <= 0 ||
  scBridgeMaxSubscriptionsPerClient <= 0 ||
  scBridgeMaxOutboundMessagesPerClient <= 0 ||
  scBridgeMaxOutboundBytesPerClient <= 0 ||
  scBridgeAuthTimeoutMs <= 0 ||
  scBridgeMaxCliQueue <= 0
) {
  throw new Error('SC-Bridge resource limits must be positive.');
}

const rpcEnabled = parseBool(
  (flags.rpc && String(flags.rpc)) || env.PEER_RPC || '',
  false
);
const rpcHost =
  (flags['rpc-host'] && String(flags['rpc-host'])) ||
  env.PEER_RPC_HOST ||
  '127.0.0.1';
const rpcPort = Number.parseInt(
  (flags['rpc-port'] && String(flags['rpc-port'])) || env.PEER_RPC_PORT || '5001',
  10
);
const rpcAllowOrigin =
  (flags['rpc-allow-origin'] && String(flags['rpc-allow-origin'])) ||
  env.PEER_RPC_ALLOW_ORIGIN ||
  '*';
const rpcMaxBodyBytes = Number.parseInt(
  (flags['rpc-max-body-bytes'] && String(flags['rpc-max-body-bytes'])) ||
    env.PEER_RPC_MAX_BODY_BYTES ||
    '1000000',
  10
);
const apiTxExposed = parseBool(
  (flags['api-tx-exposed'] && String(flags['api-tx-exposed'])) ||
    env.PEER_API_TX_EXPOSED ||
    '',
  false
);
if (scBridgeEnabled && !scBridgeToken) {
  throw new Error(
    'SC-Bridge requires --sc-bridge-token or --sc-bridge-token-file (auth is mandatory).'
  );
}
if (rpcEnabled && (!Number.isSafeInteger(rpcPort) || rpcPort < 1 || rpcPort > 65535)) {
  throw new Error('Invalid --rpc-port. Expected integer 1-65535.');
}
if (rpcEnabled && (!Number.isSafeInteger(rpcMaxBodyBytes) || rpcMaxBodyBytes < 1)) {
  throw new Error('Invalid --rpc-max-body-bytes. Expected a positive integer.');
}
if (!Number.isSafeInteger(mayhemRelayPowDifficulty) || mayhemRelayPowDifficulty < 1 || mayhemRelayPowDifficulty > 30) {
  throw new Error('Invalid --mayhem-relay-pow-difficulty. Expected integer 1-30.');
}
if (!Number.isSafeInteger(mayhemRelayMaxMessageBytes)
  || mayhemRelayMaxMessageBytes < 4_096
  || mayhemRelayMaxMessageBytes > 65_536) {
  throw new Error('Invalid --mayhem-relay-max-bytes. Expected integer 4096-65536.');
}

const peerDhtBootstrap = parseCsvList(
  (flags['peer-dht-bootstrap'] && String(flags['peer-dht-bootstrap'])) ||
    (flags['dht-bootstrap'] && String(flags['dht-bootstrap'])) ||
    env.PEER_DHT_BOOTSTRAP ||
    env.DHT_BOOTSTRAP ||
    ''
);
const msbDhtBootstrap = parseCsvList(
  (flags['msb-dht-bootstrap'] && String(flags['msb-dht-bootstrap'])) ||
    env.MSB_DHT_BOOTSTRAP ||
    ''
);
const msbWalletEnabled = parseBool(
  flagValue('msb-wallet', env.MSB_WALLET || ''),
  true
);
const msbDirectPeers = [...new Set(parseCsvList(
  (flags['msb-direct-peer'] && String(flags['msb-direct-peer'])) ||
    env.MSB_DIRECT_PEERS ||
    ''
) ?? [])];
if (msbDirectPeers.length > 16 || msbDirectPeers.some((peer) => !/^[0-9a-fA-F]{64}$/.test(peer))) {
  throw new Error('Invalid --msb-direct-peer. Expected at most 16 comma-separated 32-byte public keys.');
}
const msbDirectPeerRetryMs = parseInteger(
  flagValue('msb-direct-peer-retry-ms', env.MSB_DIRECT_PEER_RETRY_MS || ''),
  5_000
);
if (msbDirectPeerRetryMs < 1_000 || msbDirectPeerRetryMs > 300_000) {
  throw new Error('Invalid --msb-direct-peer-retry-ms. Expected 1000-300000.');
}
const msbBootstrapOverride =
  (flags['msb-bootstrap'] && String(flags['msb-bootstrap'])) ||
  env.MSB_BOOTSTRAP ||
  null;
const msbChannelOverride =
  (flags['msb-channel'] && String(flags['msb-channel'])) ||
  env.MSB_CHANNEL ||
  null;

if (networkEnv === 'testnet1' && (!msbBootstrapOverride || !msbChannelOverride)) {
  throw new Error(
    'Testnet1 requires explicit --msb-bootstrap and --msb-channel (or MSB_BOOTSTRAP/MSB_CHANNEL) so beta launches never fall back to mainnet defaults.'
  );
}

const readHexFile = (filePath, byteLength) => {
  try {
    if (fs.existsSync(filePath)) {
      const hex = fs.readFileSync(filePath, 'utf8').trim().toLowerCase();
      if (/^[0-9a-f]+$/.test(hex) && hex.length === byteLength * 2) return hex;
    }
  } catch (_e) {}
  return null;
};

const ensureKeypairFile = async (keyPairPath) => {
  if (fs.existsSync(keyPairPath)) return;
  fs.mkdirSync(path.dirname(keyPairPath), { recursive: true });
  await ensureTextCodecs();
  const wallet = new PeerWallet();
  await wallet.ready;
  if (!wallet.secretKey) {
    await wallet.generateKeyPair();
  }
  wallet.exportToFile(keyPairPath, b4a.alloc(0));
};

const subnetBootstrapFile = path.join(peerStoresDirectory, peerStoreName, 'subnet-bootstrap.hex');
let subnetBootstrap = subnetBootstrapHex ? subnetBootstrapHex.trim().toLowerCase() : null;
if (subnetBootstrap) {
  if (!/^[0-9a-f]{64}$/.test(subnetBootstrap)) {
    throw new Error('Invalid --subnet-bootstrap. Provide 32-byte hex (64 chars).');
  }
} else {
  subnetBootstrap = readHexFile(subnetBootstrapFile, 32);
}

const msbConfig = createMayhemMsbConfig(contractNetworkEnvironment, {
  storeName: msbStoreName,
  storesDirectory: msbStoresDirectory,
  enableInteractiveMode: false,
  enableWallet: msbWalletEnabled,
  ...(msbBootstrapOverride ? { bootstrap: String(msbBootstrapOverride).trim().toLowerCase() } : {}),
  ...(msbChannelOverride ? { channel: String(msbChannelOverride) } : {}),
  ...(msbDhtBootstrap ? { dhtBootstrap: msbDhtBootstrap } : {}),
});

const msbBootstrapHex = b4a.toString(msbConfig.bootstrap, 'hex');
if (subnetBootstrap && subnetBootstrap === msbBootstrapHex) {
  throw new Error('Subnet bootstrap cannot equal MSB bootstrap.');
}

const peerConfig = createMayhemPeerConfig(contractNetworkEnvironment, {
  storesDirectory: peerStoresDirectory,
  storeName: peerStoreName,
  bootstrap: subnetBootstrap || null,
  channel: subnetChannel,
  enableInteractiveMode: peerInteractive,
  enableBackgroundTasks: peerBackgroundTasks,
  enableUpdater: peerUpdater,
  replicate: peerReplicate,
  apiTxExposed,
  ...(peerDhtBootstrap ? { dhtBootstrap: peerDhtBootstrap } : {}),
});

if (msbWalletEnabled) {
  await ensureKeypairFile(msbConfig.keyPairPath);
}
await ensureKeypairFile(peerConfig.keyPairPath);

console.log('=============== STARTING MSB ===============');
const msb = new MainSettlementBus(msbConfig);
await msb.ready();

const localMsbPeer = msb.wallet?.publicKey;
const localMsbPeerHex = b4a.isBuffer(localMsbPeer)
  ? b4a.toString(localMsbPeer, 'hex')
  : String(localMsbPeer ?? '').toLowerCase();
const effectiveMsbDirectPeers = msbDirectPeers
  .map((peer) => peer.toLowerCase())
  .filter((peer) => peer !== localMsbPeerHex);
let msbDirectPeerConnectInFlight = false;
const connectMsbDirectPeers = async () => {
  if (msbDirectPeerConnectInFlight) return;
  msbDirectPeerConnectInFlight = true;
  try {
    for (const peerPublicKey of effectiveMsbDirectPeers) {
      if (msb.network.validatorConnectionManager.connected(peerPublicKey)) continue;
      await msb.network.tryConnect(peerPublicKey, 'validator');
    }
  } finally {
    msbDirectPeerConnectInFlight = false;
  }
};
if (effectiveMsbDirectPeers.length > 0) {
  await connectMsbDirectPeers();
}
const msbDirectPeerTimer = effectiveMsbDirectPeers.length > 0
  ? setInterval(() => {
      connectMsbDirectPeers().catch((error) => {
        console.error('MSB direct-peer reconnect failed:', error?.message ?? error);
      });
    }, msbDirectPeerRetryMs)
  : null;

console.log('=============== STARTING MAYHEM PEER ===============');
const peer = new Peer({
  config: peerConfig,
  msb,
  wallet: new MayhemWallet({ networkPrefix: msbConfig.addressPrefix }),
  protocol: MayhemProtocol,
  contract: MayhemContract,
});
await peer.ready();
joinCanonicalPeers(peer, peerDirectPeers);
await hydrateAdminWriterViews(peer);

let mayhemFeature = null;
{
  const admin = await peer.base.view.get('admin');
  mayhemFeature = new MayhemFeature(peer, {
    channel: MAYHEM_RELAY_CHANNEL,
    maxMessageBytes: mayhemRelayMaxMessageBytes,
    timeoutMs: mayhemRelayTimeoutMs,
    retryMs: mayhemRelayRetryMs,
    connectTimeoutMs: mayhemRelayConnectTimeoutMs,
    resultTimeoutMs: mayhemRelayResultTimeoutMs,
    resultPollMs: mayhemRelayResultPollMs,
    serviceHandler: stripeAdminService(peer),
  });
  await peer.protocol.instance.addFeature('mayhem', mayhemFeature);
  peer.mayhemFeature = mayhemFeature;
  if (admin && admin.value === peer.wallet.publicKey) {
    console.log('Mayhem Feature: ready (free admin-oracle lifecycle/evidence writer)');
  } else if (peer.base.writable) {
    console.log('Mayhem Feature: ready (free lifecycle/evidence writer)');
  } else {
    console.log('Mayhem Feature: ready (read-only until writer admission)');
  }
}

const effectiveSubnetBootstrapHex = peer.base?.key
  ? peer.base.key.toString('hex')
  : b4a.isBuffer(peer.config.bootstrap)
    ? peer.config.bootstrap.toString('hex')
    : String(peer.config.bootstrap ?? '').toLowerCase();

if (!subnetBootstrap) {
  fs.mkdirSync(path.dirname(subnetBootstrapFile), { recursive: true });
  fs.writeFileSync(subnetBootstrapFile, `${effectiveSubnetBootstrapHex}\n`);
}

const msbChannel = b4a.toString(msbConfig.channel, 'utf8');
const msbStorePath = path.join(msbStoresDirectory, msbStoreName);
const peerStorePath = path.join(peerStoresDirectory, peerStoreName);
const peerWriterKey = peer.writerLocalKey ?? peer.base?.local?.key?.toString('hex') ?? null;

console.log('');
console.log('==================== MAYHEM INTERCOM ====================');
console.log('Network:', networkEnv);
console.log('Contract version:', releaseIdentity.contractVersion);
console.log('Contract code SHA-256:', releaseIdentity.contractCodeSha256);
console.log('MSB address prefix:', msbConfig.addressPrefix);
console.log('MSB network id:', msbConfig.networkId);
console.log('MSB network bootstrap:', msbBootstrapHex);
console.log('MSB channel:', msbChannel);
console.log('MSB store:', msbStorePath);
console.log('Peer store:', peerStorePath);
if (Array.isArray(msbConfig?.dhtBootstrap) && msbConfig.dhtBootstrap.length > 0) {
  console.log('MSB DHT bootstrap nodes:', msbConfig.dhtBootstrap.join(', '));
}
if (msbDirectPeers.length > 0) {
  console.log('MSB direct replication peers:', msbDirectPeers.join(', '));
}
if (Array.isArray(peerConfig?.dhtBootstrap) && peerConfig.dhtBootstrap.length > 0) {
  console.log('Peer DHT bootstrap nodes:', peerConfig.dhtBootstrap.join(', '));
}
console.log('Peer subnet bootstrap:', effectiveSubnetBootstrapHex);
console.log('Peer subnet channel:', subnetChannel);
console.log('Peer pubkey (hex):', peer.wallet.publicKey);
console.log('Peer trac address (bech32m):', peer.wallet.address ?? null);
console.log('Peer writer key (hex):', peerWriterKey);
console.log('Sidechannel entry:', sidechannelEntry);
console.log('Mayhem relay channel:', MAYHEM_RELAY_CHANNEL);
console.log('Mayhem relay PoW difficulty:', mayhemRelayPowDifficulty);
console.log('Mayhem relay max bytes:', mayhemRelayMaxMessageBytes);
console.log('Headless:', headless);
console.log('Peer interactive:', peerInteractive);
console.log('Peer replicate:', peerReplicate);
if (sidechannelExtras.length > 0) {
  console.log('Sidechannel extras:', sidechannelExtras.join(', '));
}
if (scBridgeEnabled) {
  const portDisplay = Number.isSafeInteger(scBridgePort) ? scBridgePort : 49222;
  console.log('SC-Bridge:', `ws://${scBridgeHost}:${portDisplay}`);
}
if (inferenceRelayServe || inferenceRelayPeers.length > 0) {
  console.log('Inference relay service:', inferenceRelayServe ? 'enabled' : 'disabled');
  console.log('Inference relay peers:', inferenceRelayPeers.join(', ') || '(none)');
  console.log('Inference relay direct-first:', !inferenceRelayForce);
}
if (rpcEnabled) {
  console.log('RPC:', `http://${rpcHost}:${rpcPort}/v1`);
}
console.log('================================================================');
console.log('');

const inferenceRelay = new InferenceRelay(peer, {
  serve: inferenceRelayServe,
  relays: inferenceRelayPeers,
  forceRelay: inferenceRelayForce,
  debug: directSessionDebug,
  directWaitMs: inferenceRelayDirectWaitMs,
  relayWaitMs: inferenceRelayWaitMs,
  maxClients: inferenceRelayMaxClients,
  maxPendingPairs: inferenceRelayMaxPendingPairs,
  maxActivePairs: inferenceRelayMaxActivePairs,
  maxBytesPerLink: inferenceRelayMaxBytesPerLink,
  rateBytesPerSecond: inferenceRelayRateBytesPerSecond,
  rateBurstBytes: inferenceRelayRateBurstBytes,
  idleTimeoutMs: inferenceRelayIdleTimeoutMs,
});
await inferenceRelay.start();
peer.inferenceRelay = inferenceRelay;

let scBridge = null;
if (scBridgeEnabled) {
  scBridge = new ScBridge(peer, {
    host: scBridgeHost,
    port: Number.isSafeInteger(scBridgePort) ? scBridgePort : 49222,
    token: scBridgeToken,
    debug: scBridgeDebug,
    cliEnabled: scBridgeCliEnabled,
    requireAuth: true,
    maxClients: scBridgeMaxClients,
    maxMessageBytes: scBridgeMaxMessageBytes,
    maxSubscriptionsPerClient: scBridgeMaxSubscriptionsPerClient,
    maxOutboundMessagesPerClient: scBridgeMaxOutboundMessagesPerClient,
    maxOutboundBytesPerClient: scBridgeMaxOutboundBytesPerClient,
    authTimeoutMs: scBridgeAuthTimeoutMs,
    maxCliQueue: scBridgeMaxCliQueue,
    info: {
      app: 'mayhem',
      network: networkEnv,
      msbAddressPrefix: msbConfig.addressPrefix,
      msbNetworkId: msbConfig.networkId,
      msbBootstrap: msbBootstrapHex,
      msbChannel,
      msbStore: msbStorePath,
      peerStore: peerStorePath,
      subnetBootstrap: effectiveSubnetBootstrapHex,
      subnetChannel,
      peerPubkey: peer.wallet.publicKey,
      peerTracAddress: peer.wallet.address ?? null,
      peerWriterKey,
      sidechannelEntry,
      sidechannelExtras: sidechannelExtras.slice(),
      mayhemRelayChannel: MAYHEM_RELAY_CHANNEL,
      mayhemRelayPowDifficulty,
      mayhemRelayMaxMessageBytes,
    },
  });
}

const directSession = new DirectSession(peer, {
  debug: directSessionDebug,
  maxFrameBytes: Number.isSafeInteger(directSessionMaxFrameBytes)
    ? directSessionMaxFrameBytes
    : undefined,
  rateBytesPerSecond: Number.isSafeInteger(directSessionRateBytesPerSecond)
    ? directSessionRateBytesPerSecond
    : undefined,
  rateBurstBytes: Number.isSafeInteger(directSessionRateBurstBytes)
    ? directSessionRateBurstBytes
    : undefined,
  sendDrainTimeoutMs: Number.isSafeInteger(directSessionSendDrainTimeoutMs)
    ? directSessionSendDrainTimeoutMs
    : undefined,
  connectMaxWaitMs: Number.isSafeInteger(directSessionConnectMaxWaitMs)
    ? directSessionConnectMaxWaitMs
    : undefined,
  connectPollMs: Number.isSafeInteger(directSessionConnectPollMs)
    ? directSessionConnectPollMs
    : undefined,
  openMaxWaitMs: Number.isSafeInteger(directSessionOpenMaxWaitMs)
    ? directSessionOpenMaxWaitMs
    : undefined,
  healthIntervalMs: Number.isSafeInteger(directSessionHealthIntervalMs)
    ? directSessionHealthIntervalMs
    : undefined,
  healthFreshMs: Number.isSafeInteger(directSessionHealthFreshMs)
    ? directSessionHealthFreshMs
    : undefined,
  healthTimeoutMs: Number.isSafeInteger(directSessionHealthTimeoutMs)
    ? directSessionHealthTimeoutMs
    : undefined,
  maxSessions: Number.isSafeInteger(directSessionMaxSessions)
    ? directSessionMaxSessions
    : undefined,
  maxSessionsPerConnection: Number.isSafeInteger(directSessionMaxSessionsPerConnection)
    ? directSessionMaxSessionsPerConnection
    : undefined,
  transportInfo: (connection, remote) => inferenceRelay.connectionTransport(connection, remote),
  onFrame: scBridgeEnabled
    ? (event) => scBridge.handleSessionFrame(event)
    : null,
  onClose: scBridgeEnabled
    ? (event) => scBridge.handleSessionClose(event)
    : null,
});
peer.directSession = directSession;

const sidechannel = new Sidechannel(peer, {
  channels: [...new Set([sidechannelEntry, MAYHEM_RELAY_CHANNEL, ...sidechannelExtras])],
  debug: sidechannelDebug,
  maxMessageBytes: Number.isSafeInteger(sidechannelMaxBytes) ? sidechannelMaxBytes : undefined,
  maxMessageBytesByChannel: {
    [MAYHEM_RELAY_CHANNEL]: mayhemRelayMaxMessageBytes,
  },
  muxRetryMax: sidechannelMuxRetryMax,
  muxRetryDelayMs: sidechannelMuxRetryDelayMs,
  openRetryMax: sidechannelOpenRetryMax,
  openRetryBaseMs: sidechannelOpenRetryBaseMs,
  openRetryResetMs: sidechannelOpenRetryResetMs,
  flushTimeoutMs: sidechannelFlushTimeoutMs,
  directConnectMaxWaitMs: sidechannelDirectConnectMaxWaitMs,
  directConnectPollMs: sidechannelDirectConnectPollMs,
  maxChannels: sidechannelMaxChannels,
  maxChannelNameBytes: sidechannelMaxChannelNameBytes,
  channelOpenTimeoutMs: sidechannelChannelOpenTimeoutMs,
  powEnabled: true,
  powDifficulty: mayhemRelayPowDifficulty,
  powRequiredChannels: [MAYHEM_RELAY_CHANNEL],
  entryChannel: sidechannelEntry,
  allowRemoteOpen: sidechannelAllowRemoteOpen,
  autoJoinOnOpen: sidechannelAutoJoin,
  welcomeRequired: sidechannelWelcomeRequired,
  onMessage: (channel, payload, connection) => {
    if (mayhemFeature.isRelayMessage(payload)) {
      mayhemFeature.handleSidechannelMessage(channel, payload, connection).catch((error) => {
        console.error('Mayhem feature relay failed:', error?.message ?? error);
      });
      return;
    }
    if (scBridgeEnabled) {
      scBridge.handleSidechannelMessage(channel, payload, connection);
    } else if (!sidechannelQuiet) {
      const from = payload?.from ?? 'unknown';
      console.log(`[sidechannel:${channel}] ${from}:`, payload?.message ?? payload);
    }
  },
});
peer.sidechannel = sidechannel;

if (scBridge) {
  scBridge.attachSidechannel(sidechannel);
  scBridge.attachDirectSession(directSession);
  scBridge.attachInferenceRelay(inferenceRelay);
  scBridge.start();
  peer.scBridge = scBridge;
}

let rpcServer = null;
if (rpcEnabled) {
  rpcServer = createRpcServer(peer, {
    maxBodyBytes: rpcMaxBodyBytes,
    allowOrigin: rpcAllowOrigin,
    releaseIdentity,
    statusMetadata: {
      subnetBootstrapHex: effectiveSubnetBootstrapHex,
      subnetChannelUtf8: subnetChannel,
      peerDhtBootstrap: peerConfig.dhtBootstrap,
      msbChannel,
      msbDhtBootstrap: msbConfig.dhtBootstrap,
    },
  });
  rpcServer.listen(rpcPort, rpcHost, () => {
    console.log('RPC: ready', `http://${rpcHost}:${rpcPort}/v1`);
  });
  rpcServer.on('error', (error) => fatalRuntimeError('RPC server error', error));
  peer.rpcServer = rpcServer;
}

directSession.start();
await sidechannel.start();
console.log('Sidechannel: ready');

if (headless) {
  console.log('Terminal: disabled (headless)');
} else {
  const terminal = new Terminal(peer);
  await terminal.start();
}

if (keepAlive) {
  const close = async () => {
    if (msbDirectPeerTimer) clearInterval(msbDirectPeerTimer);
    try {
      rpcServer?.close?.();
    } catch (_e) {}
    try {
      scBridge?.stop?.();
    } catch (_e) {}
    try {
      await inferenceRelay.stop();
    } catch (_e) {}
    try {
      await peer.close?.();
    } catch (_e) {}
    try {
      await msb.close?.();
    } catch (_e) {}
    if (typeof Bare !== 'undefined' && Bare.exit) Bare.exit(0);
  };
  if (typeof Pear !== 'undefined' && Pear.teardown) Pear.teardown(close);
  await new Promise(() => {});
}
