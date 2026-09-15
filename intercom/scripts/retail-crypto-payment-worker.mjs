#!/usr/bin/env node
import { spawn } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { MainSettlementBus } from 'trac-msb/src/index.js';

import { TAP_DEPOSIT_EVENT_SIGNATURE, tapDepositKey } from '../../contracts/scripts/tap-deposit-watcher.mjs';
import { createLocalConfig, sleep } from './msb-local-common.mjs';
import {
  ERC20_TRANSFER_TOPIC,
  RetryWork,
  ReviewWork,
  addressTopic,
  normalizeHex,
  normalizeHex64,
  normalizeTnkAddress,
  parseHexInt,
  summarizePayoutLiabilities,
  uniqueIntentByAmount,
  validateTapBridgePreflight,
  verifyTapTransferReceipt,
} from './retail-crypto-verification.mjs';
import { scanMsbTransfers } from './tnk-deposit-watcher.mjs';

const scriptPath = fileURLToPath(import.meta.url);
const repoRoot = path.resolve(path.dirname(scriptPath), '../..');
const TOKEN_SCALE = 1_000_000_000_000_000_000n;

function requiredEnv(name, env = process.env) {
  const value = String(env[name] ?? '').trim();
  if (!value) throw new Error(`Missing ${name}`);
  return value;
}

function positiveInt(value, fallback, label) {
  const parsed = Number(value ?? fallback);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) throw new Error(`${label} must be positive`);
  return parsed;
}

function atomicJson(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
  const temporary = `${file}.tmp-${process.pid}-${Date.now()}`;
  fs.writeFileSync(temporary, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
  fs.renameSync(temporary, file);
  fs.chmodSync(file, 0o600);
}

function readJson(file, fallback = {}) {
  if (!fs.existsSync(file)) return fallback;
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

async function closeMsb(msb) {
  await Promise.race([
    Promise.resolve().then(() => msb.close()).catch(() => undefined),
    sleep(10_000),
  ]);
}

function sha256(value) {
  return createHash('sha256').update(String(value)).digest('hex');
}

function ceilDiv(value, divisor) {
  return (value + divisor - 1n) / divisor;
}

function jsonOutput(stdout) {
  const text = String(stdout ?? '').trim();
  try { return JSON.parse(text); } catch {}
  const first = text.indexOf('{');
  const last = text.lastIndexOf('}');
  if (first === -1 || last <= first) throw new Error('command returned no JSON report');
  return JSON.parse(text.slice(first, last + 1));
}

class WorkerApi {
  constructor(baseUrl, secret, workerId) {
    this.base = new URL(baseUrl.endsWith('/') ? baseUrl : `${baseUrl}/`);
    this.secret = secret;
    this.workerId = workerId;
  }

  async post(relative, body) {
    const response = await fetch(new URL(relative, this.base), {
      method: 'POST',
      headers: { authorization: `Bearer ${this.secret}`, 'content-type': 'application/json' },
      body: JSON.stringify(body),
      signal: AbortSignal.timeout(15_000),
    });
    const payload = await response.json().catch(() => null);
    if (!response.ok) throw new Error(`worker API ${relative} failed with HTTP ${response.status}`);
    return payload;
  }

  pull() { return this.post('internal/crypto-payment-worker/pull', { worker_id: this.workerId }); }
  discovery(rail) {
    return this.post('internal/crypto-payment-worker/discovery', { worker_id: this.workerId, rail });
  }
  discovered(intent, transactionHash, observedAt) {
    return this.post(`internal/crypto-payment-worker/${intent.id}/discover`, {
      worker_id: this.workerId,
      transaction_hash: transactionHash,
      observed_at: observedAt.toISOString(),
    });
  }
  renew(work) { return this.post(`internal/crypto-payment-worker/${work.intent.id}/renew`, { lease_token: work.lease_token }); }
  retry(work, error) {
    return this.post(`internal/crypto-payment-worker/${work.intent.id}/retry`, {
      lease_token: work.lease_token,
      code: error.code,
      retry_after_seconds: error.delaySeconds,
      transfer_observed: error.transferObserved,
    });
  }
  review(work, reason, evidence = null) {
    return this.post(`internal/crypto-payment-worker/${work.intent.id}/review`, {
      lease_token: work.lease_token,
      reason,
      ...(evidence ? { evidence: serializeEvidence(evidence) } : {}),
    });
  }
  evidence(work, evidence) {
    return this.post(`internal/crypto-payment-worker/${work.intent.id}/evidence`, {
      lease_token: work.lease_token,
      evidence: serializeEvidence({ ...evidence, intentId: work.intent.id }),
    });
  }
  status(report) { return this.post('internal/crypto-payment-worker/status', report); }
}

function serializeEvidence(value) {
  return JSON.parse(JSON.stringify(value, (_, item) => typeof item === 'bigint' ? item.toString() : item));
}

class TapRpc {
  constructor(urls, expectedChainId) {
    this.urls = urls;
    this.expectedChainId = BigInt(expectedChainId);
    this.selected = null;
    this.selectedIndex = null;
    this.nextId = 1;
  }

  async select() {
    for (const [index, url] of this.urls.entries()) {
      try {
        const chainId = parseHexInt(await this.callUrl(url, 'eth_chainId', []), 'Ethereum chain id');
        if (chainId === this.expectedChainId) {
          this.selected = url;
          this.selectedIndex = index;
          return;
        }
      } catch {}
    }
    throw new RetryWork('rpc_unavailable', 30);
  }

  async call(method, params) {
    if (!this.selected) await this.select();
    try {
      return await this.callUrl(this.selected, method, params);
    } catch {
      this.selected = null;
      this.selectedIndex = null;
      await this.select();
      return this.callUrl(this.selected, method, params);
    }
  }

  async callUrl(url, method, params) {
    const response = await fetch(url, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '2.0', id: this.nextId++, method, params }),
      signal: AbortSignal.timeout(10_000),
    });
    const body = await response.json().catch(() => null);
    if (!response.ok || body?.error || !Object.hasOwn(body ?? {}, 'result')) throw new Error('Ethereum RPC failed');
    return body.result;
  }

  mode() { return this.selectedIndex === 0 ? 'primary' : 'fallback'; }
}

function coreStateUrl(baseUrl, key, { prefix = false, signedLength } = {}) {
  const base = new URL(baseUrl.endsWith('/') ? baseUrl : `${baseUrl}/`);
  const url = new URL('state', base);
  url.searchParams.set(prefix ? 'prefix' : 'key', key);
  url.searchParams.set('confirmed', 'true');
  if (prefix) url.searchParams.set('limit', '1000');
  if (signedLength !== undefined) url.searchParams.set('signed_length', String(signedLength));
  return url;
}

async function readCore(baseUrl, key, options = {}) {
  const response = await fetch(coreStateUrl(baseUrl, key, options), { signal: AbortSignal.timeout(10_000) });
  const body = await response.json().catch(() => null);
  if (!response.ok || body?.confirmed !== true || !Number.isSafeInteger(body?.signed_length)) {
    throw new RetryWork('core_unavailable', 30);
  }
  if (options.signedLength !== undefined && body.signed_length !== options.signedLength) {
    throw new RetryWork('core_unavailable', 30);
  }
  return body;
}

async function coreMsbSignedLength(baseUrl) {
  const base = new URL(baseUrl.endsWith('/') ? baseUrl : `${baseUrl}/`);
  const response = await fetch(new URL('status', base), { signal: AbortSignal.timeout(10_000) });
  const body = await response.json().catch(() => null);
  const value = Number(body?.msb?.signedLength);
  if (!response.ok || !Number.isSafeInteger(value) || value <= 0) throw new RetryWork('core_unavailable', 30);
  return value;
}

async function coreWorkingFunds(config, rail) {
  const name = rail.toLowerCase();
  const balance = await readCore(config.coreRpc, `bal/${config.platformBuyer}/${name}`);
  const signedLength = balance.signed_length;
  const holds = await Promise.all([
    'targeted-outstanding', 'targeted-summary', 'targeted-legacy-release',
  ].map((kind) => readCore(config.coreRpc, `hold/${kind}/${name}/${config.platformBuyer}`, { signedLength })));
  const value = balance.value;
  if (!value || String(value.user).toLowerCase() !== config.platformBuyer || value.rail !== name ||
      value.denom !== 'au_usd' || !/^\d+$/.test(String(value.au))) {
    throw new RetryWork('core_unavailable', 30);
  }
  const amount = (record, field) => {
    if (!record.value) return 0n;
    const text = String(record.value[field] ?? '');
    if (!/^\d+$/.test(text)) throw new RetryWork('core_unavailable', 30);
    return BigInt(text);
  };
  const legacy = amount(holds[0], 'reserved_au');
  const summary = amount(holds[1], 'reserved_au');
  const released = amount(holds[2], 'released_au');
  if (released > legacy) throw new RetryWork('core_unavailable', 30);
  return { balanceAu: BigInt(value.au), heldAu: legacy - released + summary, signedLength };
}

function exactEpoch(values, prefix) {
  let latest = 0;
  for (const entry of values ?? []) {
    const tail = String(entry?.key ?? '').slice(prefix.length);
    if (/^[1-9][0-9]*$/.test(tail)) latest = Math.max(latest, Number(tail));
  }
  return latest;
}

async function coreSettlementState(config, rail) {
  const name = rail.toLowerCase();
  const settlementPrefix = `settle/targeted/${name}/`;
  const [liabilityRecords, settlementRecords, applyRecords] = await Promise.all([
    readCore(config.coreRpc, `payout/liability/${name}/`, { prefix: true }),
    readCore(config.coreRpc, settlementPrefix, { prefix: true }),
    readCore(config.coreRpc, 'epoch/apply-anchor/', { prefix: true }),
  ]);
  if (liabilityRecords.truncated || settlementRecords.truncated || applyRecords.truncated) {
    throw new RetryWork('settlement_state_truncated', 60);
  }
  const liabilities = summarizePayoutLiabilities(liabilityRecords.values ?? [], rail);
  const currentEpoch = exactEpoch(applyRecords.values, 'epoch/apply-anchor/');
  const lastSettledEpoch = exactEpoch(settlementRecords.values, settlementPrefix);
  const lag = Math.max(0, currentEpoch - lastSettledEpoch);
  const payoutStatus = liabilities.payableAu === 0n
    ? 'current'
    : lag <= config.settlementMaxEpochLag ? 'pending' : 'lagging';
  return {
    ...liabilities,
    currentEpoch,
    lastSettledEpoch,
    payoutStatus,
    lag,
  };
}

function erc20BalanceCall(address) {
  return `0x70a08231${addressTopic(address).slice(2)}`;
}

async function tapOperationalStatus(config) {
  const rpc = new TapRpc(config.tapRpcUrls, config.tapChainId);
  await rpc.select();
  const payments = await readCore(config.coreRpc, 'payments/current');
  const rate = await readCore(config.coreRpc, 'tap/rate/latest');
  const pool = normalizeHex(payments.value?.tap?.pool_address, 20, 'TAP pool');
  const rateAu = BigInt(String(rate.value?.tap_usd_au ?? '0'));
  if (rateAu <= 0n) throw new RetryWork('rate_unavailable', 30);
  const [{ latestNumber, finalizedNumber }, tokenBalance, gasBalance, settlementGasBalance,
    poolBalance, core, payout] = await Promise.all([
    tapFinality(rpc),
    rpc.call('eth_call', [{ to: config.tapToken, data: erc20BalanceCall(config.tapCollection) }, 'latest']),
    rpc.call('eth_getBalance', [config.tapCollection, 'latest']),
    rpc.call('eth_getBalance', [config.tapSettlementGasAddress, 'latest']),
    rpc.call('eth_call', [{ to: config.tapToken, data: erc20BalanceCall(pool) }, 'latest']),
    coreWorkingFunds(config, 'TAP'),
    coreSettlementState(config, 'TAP'),
  ]);
  const collectionGasWei = parseHexInt(gasBalance, 'TAP collection gas balance');
  const settlementGasWei = parseHexInt(settlementGasBalance, 'TAP settlement gas balance');
  const poolBalanceWei = parseHexInt(poolBalance, 'TAP pool balance');
  const requiredWei = ceilDiv(payout.unsettledAu * TOKEN_SCALE, rateAu);
  const gasReady = collectionGasWei >= config.tapMinimumGasWei &&
    settlementGasWei >= config.tapMinimumGasWei;
  const assetsReady = poolBalanceWei >= requiredWei;
  const settlementReady = payout.payoutStatus !== 'lagging' && gasReady && assetsReady;
  return {
    worker_id: config.workerId,
    rail: 'TAP',
    reader_healthy: true,
    rpc_mode: rpc.mode(),
    collection_balance_base_units: parseHexInt(tokenBalance, 'TAP collection balance').toString(),
    gas_balance_base_units: collectionGasWei.toString(),
    external_height: latestNumber.toString(),
    external_finalized_height: finalizedNumber.toString(),
    core_balance_au: core.balanceAu.toString(),
    core_held_au: core.heldAu.toString(),
    core_signed_length: String(core.signedLength),
    settlement: {
      ready: settlementReady,
      payout_status: payout.payoutStatus,
      current_epoch: payout.currentEpoch,
      last_settled_epoch: payout.lastSettledEpoch || null,
      epoch_lag: payout.lag,
      liability_au: payout.unsettledAu.toString(),
      held_au: payout.heldAu.toString(),
      payable_au: payout.payableAu.toString(),
      pool_balance_base_units: poolBalanceWei.toString(),
      required_base_units: requiredWei.toString(),
      sponsorship_ready: true,
      gas_ready: gasReady,
      settlement_gas_balance_base_units: settlementGasWei.toString(),
      assets_ready: assetsReady,
    },
    last_error: null,
    checked_at: new Date().toISOString(),
  };
}

async function tnkOperationalStatus(config) {
  const msbConfig = createLocalConfig({
    network: config.tnkNetwork,
    stateDir: path.join(config.stateDir, 'tnk-health-reader'),
    storeName: `${config.tnkReaderStore}-health`,
    channel: process.env.MSB_CHANNEL || undefined,
    bootstrap: process.env.MSB_BOOTSTRAP || undefined,
    enableWallet: false,
  });
  const msb = new MainSettlementBus(msbConfig);
  try {
    await Promise.race([
      msb.ready(),
      sleep(config.readerTimeoutSeconds * 1_000).then(() => { throw new RetryWork('reader_unavailable', 30); }),
    ]);
    const [payments, rate, payout] = await Promise.all([
      readCore(config.coreRpc, 'payments/current'),
      readCore(config.coreRpc, 'rate/latest'),
      coreSettlementState(config, 'TNK'),
    ]);
    const treasuryAddress = normalizeTnkAddress(
      payments.value?.tnk?.treasury_address,
      config.tnkNetwork,
      'TNK treasury address',
    );
    const rateAu = BigInt(String(rate.value?.tnk_usd_au ?? '0'));
    if (rateAu <= 0n) throw new RetryWork('rate_unavailable', 30);
    let balance = null;
    let treasuryBalance = null;
    for (let waited = 0; waited <= config.readerTimeoutSeconds; waited += 1) {
      balance = await msb.getBalance(config.tnkCollection, true);
      treasuryBalance = await msb.getBalance(treasuryAddress, true);
      if (balance && treasuryBalance) break;
      await sleep(1_000);
    }
    if (!balance || !treasuryBalance || !/^\d+$/.test(String(balance.balance)) ||
        !/^\d+$/.test(String(treasuryBalance.balance))) throw new RetryWork('reader_unavailable', 30);
    const core = await coreWorkingFunds(config, 'TNK');
    const requiredE18 = ceilDiv(payout.unsettledAu * TOKEN_SCALE, rateAu);
    const treasuryE18 = BigInt(treasuryBalance.balance);
    const sponsorshipReady = treasuryE18 >= requiredE18;
    const settlementReady = payout.payoutStatus !== 'lagging' && sponsorshipReady;
    return {
      worker_id: config.workerId,
      rail: 'TNK',
      reader_healthy: true,
      rpc_mode: 'msb',
      collection_balance_base_units: String(balance.balance),
      gas_balance_base_units: null,
      external_height: String(msb.state.getSignedLength()),
      external_finalized_height: String(Math.max(0, msb.state.getSignedLength() - config.tnkFinality)),
      core_balance_au: core.balanceAu.toString(),
      core_held_au: core.heldAu.toString(),
      core_signed_length: String(core.signedLength),
      settlement: {
        ready: settlementReady,
        payout_status: payout.payoutStatus,
        current_epoch: payout.currentEpoch,
        last_settled_epoch: payout.lastSettledEpoch || null,
        epoch_lag: payout.lag,
        liability_au: payout.unsettledAu.toString(),
        held_au: payout.heldAu.toString(),
        payable_au: payout.payableAu.toString(),
        treasury_balance_base_units: treasuryE18.toString(),
        required_base_units: requiredE18.toString(),
        sponsorship_ready: sponsorshipReady,
        gas_ready: true,
        assets_ready: sponsorshipReady,
      },
      last_error: null,
      checked_at: new Date().toISOString(),
    };
  } finally {
    await closeMsb(msb);
  }
}

async function reportOperationalStatus(config) {
  for (const [rail, collect] of [['TAP', tapOperationalStatus], ['TNK', tnkOperationalStatus]]) {
    try {
      await config.api.status(await collect(config));
    } catch (error) {
      const detail = String(error?.message ?? error)
        .replace(/https?:\/\/[^\s"']+/g, '<redacted-endpoint>')
        .slice(0, 500);
      console.error(JSON.stringify({
        event: 'crypto_payment_status_unavailable',
        rail,
        code: error instanceof RetryWork ? error.code : 'status_unavailable',
        detail,
      }));
      const core = await coreWorkingFunds(config, rail).catch(() => ({ balanceAu: 0n, heldAu: 0n, signedLength: 0 }));
      await config.api.status({
        worker_id: config.workerId,
        rail,
        reader_healthy: false,
        rpc_mode: rail === 'TNK' ? 'msb' : null,
        core_balance_au: core.balanceAu.toString(),
        core_held_au: core.heldAu.toString(),
        core_signed_length: String(core.signedLength),
        settlement: null,
        last_error: error instanceof RetryWork ? error.code : 'status_unavailable',
        checked_at: new Date().toISOString(),
      }).catch(() => undefined);
    }
  }
}

async function runCommand(command, args, { timeoutMs = 1_200_000 } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd: repoRoot, env: process.env, stdio: ['ignore', 'pipe', 'pipe'] });
    let stdout = '';
    let stderr = '';
    const append = (current, chunk) => (current + chunk.toString('utf8')).slice(-4_000_000);
    child.stdout.on('data', (chunk) => { stdout = append(stdout, chunk); });
    child.stderr.on('data', (chunk) => { stderr = append(stderr, chunk); });
    const timeout = setTimeout(() => child.kill('SIGTERM'), timeoutMs);
    child.on('error', (error) => { clearTimeout(timeout); reject(error); });
    child.on('close', (code, signal) => {
      clearTimeout(timeout);
      if (code === 0) resolve({ stdout, stderr });
      else reject(new Error(`command exited ${code ?? signal ?? 'unknown'}`));
    });
  });
}

async function tapFinality(rpc) {
  const latest = await rpc.call('eth_blockNumber', []);
  let finalized = null;
  try { finalized = await rpc.call('eth_getBlockByNumber', ['finalized', false]); } catch {}
  const latestNumber = parseHexInt(latest, 'latest block');
  const finalizedNumber = finalized?.number
    ? parseHexInt(finalized.number, 'finalized block')
    : latestNumber > 12n ? latestNumber - 12n : 0n;
  return { latestNumber, finalizedNumber };
}

async function tapBlockTime(rpc, blockNumber) {
  const block = await rpc.call('eth_getBlockByNumber', [blockNumber, false]);
  const timestamp = parseHexInt(block?.timestamp, 'TAP block timestamp');
  return new Date(Number(timestamp) * 1_000);
}

async function discoverTapIncoming(config, intents) {
  if (intents.length === 0) return;
  const rpc = new TapRpc(config.tapRpcUrls, config.tapChainId);
  await rpc.select();
  const latest = parseHexInt(await rpc.call('eth_blockNumber', []), 'latest block');
  const checkpointFile = config.discoveryCheckpointFile('tap');
  const checkpoint = readJson(checkpointFile, {});
  const stored = /^\d+$/.test(String(checkpoint.next_block ?? ''))
    ? BigInt(checkpoint.next_block)
    : null;
  let start = stored === null || stored > latest
    ? (latest > BigInt(config.tapBridgeLookbackBlocks) ? latest - BigInt(config.tapBridgeLookbackBlocks) : 0n)
    : (stored > 12n ? stored - 12n : 0n);
  const claimed = new Set();
  while (start <= latest) {
    const end = start + 999n < latest ? start + 999n : latest;
    const logs = await rpc.call('eth_getLogs', [{
      address: config.tapToken,
      fromBlock: `0x${start.toString(16)}`,
      toBlock: `0x${end.toString(16)}`,
      topics: [ERC20_TRANSFER_TOPIC, null, addressTopic(config.tapCollection)],
    }]);
    for (const log of Array.isArray(logs) ? logs : []) {
      const intent = uniqueIntentByAmount(
        intents.filter((candidate) => !claimed.has(candidate.id)),
        parseHexInt(log.data, 'TAP transfer amount'),
      );
      if (!intent) continue;
      const observedAt = await tapBlockTime(rpc, log.blockNumber);
      await config.api.discovered(intent, normalizeHex(log.transactionHash, 32, 'TAP transaction'), observedAt);
      claimed.add(intent.id);
    }
    atomicJson(checkpointFile, { next_block: (end + 1n).toString(), updated_at: new Date().toISOString() });
    start = end + 1n;
  }
}

async function discoverTnkIncoming(config, intents) {
  if (intents.length === 0) return;
  const minimumSignedLength = await coreMsbSignedLength(config.coreRpc);
  const msbConfig = createLocalConfig({
    network: config.tnkNetwork,
    stateDir: path.join(config.stateDir, 'tnk-discovery-reader'),
    storeName: config.tnkDiscoveryReaderStore,
    channel: process.env.MSB_CHANNEL || undefined,
    bootstrap: process.env.MSB_BOOTSTRAP || undefined,
    dhtBootstrap: process.env.MSB_DHT_BOOTSTRAP || undefined,
    enableWallet: false,
  });
  const msb = new MainSettlementBus(msbConfig);
  try {
    await Promise.race([
      msb.ready(),
      sleep(config.readerTimeoutSeconds * 1_000).then(() => { throw new RetryWork('reader_unavailable', 30); }),
    ]);
    const checkpointFile = config.discoveryCheckpointFile('tnk');
    const checkpoint = readJson(checkpointFile, {});
    const current = msb.state.getSignedLength();
    const stored = Number(checkpoint.next_signed_length);
    const start = Number.isSafeInteger(stored) && stored >= 0 && stored <= current
      ? Math.max(0, stored - Math.max(2, config.tnkFinality))
      : Math.max(0, current - config.tnkLookback);
    // Discovery observes signed transfers immediately; the verification pass
    // still enforces the configured finality before any credit is posted.
    const scan = await scanMsbTransfers(msb, {
      fromSignedLength: start,
      finalitySignedLengths: 0,
      chunkSize: 500,
      timeoutSec: config.readerTimeoutSeconds,
      minimumSignedLength,
    });
    const claimed = new Set();
    for (const transfer of scan.transfers) {
      if (String(transfer.to).toLowerCase() !== config.tnkCollection) continue;
      const intent = uniqueIntentByAmount(
        intents.filter((candidate) => !claimed.has(candidate.id)),
        transfer.tnk_e18,
      );
      if (!intent) continue;
      await config.api.discovered(intent, normalizeHex64(transfer.hash, 'TNK transaction'), new Date());
      claimed.add(intent.id);
    }
    atomicJson(checkpointFile, {
      next_signed_length: scan.confirmedLength,
      updated_at: new Date().toISOString(),
    });
  } finally {
    await closeMsb(msb);
  }
}

async function discoverIncoming(config) {
  for (const rail of ['TAP', 'TNK']) {
    try {
      const intents = (await config.api.discovery(rail))?.intents ?? [];
      if (rail === 'TAP') await discoverTapIncoming(config, intents);
      else await discoverTnkIncoming(config, intents);
    } catch (error) {
      const detail = String(error?.message ?? error)
        .replace(/https?:\/\/\S+/g, '<url>')
        .slice(0, 300);
      console.error(JSON.stringify({
        event: 'crypto_payment_discovery_unavailable', rail, detail,
      }));
    }
  }
}

async function verifyTapCustomerTransfer(intent, rpc) {
  const receipt = await rpc.call('eth_getTransactionReceipt', [intent.transaction_hash]);
  const { latestNumber, finalizedNumber } = await tapFinality(rpc);
  return verifyTapTransferReceipt(receipt, {
    transactionHash: intent.transaction_hash,
    token: intent.token_contract,
    destination: intent.destination,
    amountBaseUnits: intent.token_amount_base_units,
    latestBlock: latestNumber,
    finalizedBlock: finalizedNumber,
  });
}

async function verifyTnkCustomerTransfer(intent, config) {
  const minimumSignedLength = await coreMsbSignedLength(config.coreRpc);
  const hash = normalizeHex64(intent.transaction_hash, 'TNK transaction hash');
  const msbConfig = createLocalConfig({
    network: config.tnkNetwork,
    stateDir: path.join(config.stateDir, 'tnk-reader'),
    storeName: config.tnkReaderStore,
    channel: process.env.MSB_CHANNEL || undefined,
    bootstrap: process.env.MSB_BOOTSTRAP || undefined,
    dhtBootstrap: process.env.MSB_DHT_BOOTSTRAP || undefined,
    enableWallet: false,
  });
  const msb = new MainSettlementBus(msbConfig);
  try {
    await Promise.race([
      msb.ready(),
      sleep(config.readerTimeoutSeconds * 1_000).then(() => { throw new RetryWork('reader_unavailable', 30); }),
    ]);
    const confirmed = msb.state.getSignedLength();
    const scan = await scanMsbTransfers(msb, {
      fromSignedLength: Math.max(0, confirmed - config.tnkLookback),
      finalitySignedLengths: config.tnkFinality,
      chunkSize: 500,
      timeoutSec: config.readerTimeoutSeconds,
      minimumSignedLength,
      matchHash: hash,
    });
    const transfer = scan.transfers.find((candidate) => candidate.hash === hash);
    if (!transfer) throw new RetryWork('transfer_pending', 20);
    const confirmations = Math.max(0, scan.confirmedLength - Number(transfer.confirmed_length) + 1);
    const evidence = {
      rail: 'TNK',
      transactionHash: hash,
      logIndex: -1,
      blockNumber: BigInt(transfer.confirmed_length),
      blockHash: sha256(`msb/${config.tnkNetwork}/${transfer.confirmed_length}`),
      fromAddress: String(transfer.from),
      toAddress: String(transfer.to),
      tokenAmountBaseUnits: BigInt(transfer.tnk_e18),
      confirmations,
      finalized: Number(transfer.confirmed_length) <= scan.safeEnd,
      observedAt: new Date(),
      externalRecordKey: `tnk/${config.tnkNetwork}/${hash}`,
    };
    if (!evidence.finalized) throw new RetryWork('awaiting_finality', 20, true);
    if (String(transfer.to).toLowerCase() !== String(intent.destination).toLowerCase()) {
      throw new ReviewWork('wrong_destination', evidence);
    }
    if (BigInt(transfer.tnk_e18) !== BigInt(intent.token_amount_base_units)) {
      throw new ReviewWork('amount_mismatch', evidence);
    }
    return evidence;
  } finally {
    await closeMsb(msb);
  }
}

function auToUsd(au) {
  // Core's TNK deposit CLI accepts cents. Round the backing lot upward so
  // every micro-valued retail credit is fully covered; the sub-cent remainder
  // stays in the platform buyer's TNK balance for later purchases.
  const centAu = 10_000_000_000_000_000n;
  const cents = (BigInt(au) + centAu - 1n) / centAu;
  const whole = cents / 100n;
  const fraction = (cents % 100n).toString().padStart(2, '0');
  return `${whole}.${fraction}`;
}

function e18ToDecimal(amount) {
  const value = BigInt(amount);
  if (value <= 0n) throw new RetryWork('core_unavailable', 60, true);
  const whole = value / TOKEN_SCALE;
  const fraction = (value % TOKEN_SCALE).toString().padStart(18, '0').replace(/0+$/, '');
  return fraction ? `${whole}.${fraction}` : whole.toString();
}

async function waitForCoreRecord(config, work, key, expectedAu, timeoutSeconds = 600) {
  const deadline = Date.now() + timeoutSeconds * 1_000;
  while (Date.now() <= deadline) {
    const record = await readCore(config.coreRpc, key);
    const value = record.value;
    if (value && typeof value.au === 'string' && BigInt(value.au) >= BigInt(expectedAu)) {
      return { value, signedLength: BigInt(record.signed_length), key };
    }
    await config.api.renew(work);
    await sleep(5_000);
  }
  throw new RetryWork('core_bridge_pending', 30, true);
}

async function bridgeTnk(config, work, checkpoint) {
  const intent = work.intent;
  const nonce = sha256(`openmayhem-retail-tnk-v1/${intent.id}`);
  const common = [
    'pay', 'tnk', '--amount', auToUsd(intent.expected_core_au), '--nonce', nonce,
    '--home', config.buyerHome, '--rpc-url', config.coreRpc,
    '--wallet-password-file', config.walletPasswordFile, '--json',
  ];
  const dry = jsonOutput((await runCommand(config.mayhemBin, common, { api: config.api, work })).stdout);
  if (String(dry.who).toLowerCase() !== config.platformBuyer) throw new RetryWork('core_unavailable', 60, true);
  const memo = normalizeHex64(dry.memo_hash, 'TNK bridge memo');
  checkpoint.tnk_memo_hash = memo;
  atomicJson(config.checkpointFile(intent.id), checkpoint);
  const creditedKey = `dep/tnk-credited/${memo}`;
  let credited = await readCore(config.coreRpc, creditedKey);
  if (!credited.value) {
    const pendingKey = `dep/pending/${memo}`;
    let pending = await readCore(config.coreRpc, pendingKey);
    if (!pending.value) {
      await runCommand(config.mayhemBin, [...common.slice(0, -1), '--submit-intent', '--json'], { api: config.api, work });
      const deadline = Date.now() + config.bridgeTimeoutSeconds * 1_000;
      while (!pending.value && Date.now() <= deadline) {
        await config.api.renew(work);
        await sleep(5_000);
        pending = await readCore(config.coreRpc, pendingKey);
      }
    }
    const locked = pending.value;
    if (!locked || String(locked.user).toLowerCase() !== config.platformBuyer ||
        String(locked.msb_network).toLowerCase() !== config.tnkNetwork ||
        normalizeTnkAddress(locked.msb_from, config.tnkNetwork, 'TNK bridge sender') !== config.tnkCollection ||
        normalizeTnkAddress(locked.treasury_address, config.tnkNetwork, 'TNK bridge treasury') !==
          normalizeTnkAddress(dry.treasury_address, config.tnkNetwork, 'TNK bridge treasury') ||
        BigInt(locked.quoted_au ?? 0) < BigInt(intent.expected_core_au)) {
      throw new RetryWork('core_unavailable', 60, true);
    }
    const lockedTnkE18 = BigInt(locked.tnk_e18 ?? 0);
    const transferArgs = [
      path.join(repoRoot, 'crates/mayhem-cli/src/msb-transfer-helper.mjs'),
      'settlement-transfer', '--network', config.tnkNetwork,
      '--stores-directory', config.msbStoresDirectory, '--store-name', config.msbStoreName,
      '--to', locked.treasury_address, '--amount', e18ToDecimal(lockedTnkE18),
      '--operation-id', sha256(`openmayhem-retail-tnk-bridge-v2/${intent.id}/${lockedTnkE18}`),
      '--journal-file', path.join(config.stateDir, 'tnk-journals', `${intent.id}-${lockedTnkE18}.json`),
      '--wallet-password-file', config.walletPasswordFile,
      '--timeout-seconds', String(config.bridgeTimeoutSeconds),
    ];
    await runCommand(process.execPath, transferArgs, { api: config.api, work });
    credited = await readCore(config.coreRpc, creditedKey);
  }
  const record = credited.value
    ? { value: credited.value, signedLength: BigInt(credited.signed_length), key: creditedKey }
    : await waitForCoreRecord(config, work, creditedKey, intent.expected_core_au, config.bridgeTimeoutSeconds);
  if (String(record.value.user).toLowerCase() !== config.platformBuyer ||
      BigInt(record.value.au) < BigInt(intent.expected_core_au)) {
    throw new RetryWork('core_bridge_pending', 30, true);
  }
  checkpoint.core_record_key = record.key;
  atomicJson(config.checkpointFile(intent.id), checkpoint);
  return { backed: true, buyer: config.platformBuyer, rail: 'TNK', creditedAu: BigInt(record.value.au),
    confirmedSignedLength: record.signedLength, recordKey: record.key };
}

function tapDust(intentId) {
  // Keep deposits unique without materially changing what the platform funds.
  // Ten hex digits provide about one trillion distinct wei values while the
  // maximum surcharge stays below 0.0000011 TAP.
  return BigInt(`0x${sha256(`openmayhem-retail-tap-dust-v1/${intentId}`).slice(0, 10)}`) + 1n;
}

async function findTapDeposit(rpc, pool, buyer, amountWei, lookbackBlocks) {
  const latestHex = await rpc.call('eth_blockNumber', []);
  const latest = parseHexInt(latestHex, 'latest block');
  const from = latest > BigInt(lookbackBlocks) ? latest - BigInt(lookbackBlocks) : 0n;
  const logs = await rpc.call('eth_getLogs', [{
    address: pool,
    fromBlock: `0x${from.toString(16)}`,
    toBlock: 'latest',
    topics: [TAP_DEPOSIT_EVENT_SIGNATURE, addressTopic(buyer)],
  }]);
  const exact = (Array.isArray(logs) ? logs : []).filter((log) => parseHexInt(log.data, 'TAP deposit amount') === amountWei);
  if (exact.length > 1) throw new ReviewWork('duplicate_transfer');
  return exact[0] ?? null;
}

async function bridgeTap(config, work, checkpoint, rpc) {
  const intent = work.intent;
  if (!rpc.selected) await rpc.select();
  const [payments, rate] = await Promise.all([
    readCore(config.coreRpc, 'payments/current'),
    readCore(config.coreRpc, 'tap/rate/latest'),
  ]);
  const tap = payments.value?.tap;
  const rateAu = BigInt(String(rate.value?.tap_usd_au ?? '0'));
  if (!tap || Number(tap.chain_id) !== config.tapChainId || rateAu <= 0n ||
      String(tap.token_address).toLowerCase() !== String(intent.token_contract).toLowerCase()) {
    throw new RetryWork('core_unavailable', 60, true);
  }
  const calculatedAmountWei = ceilDiv(BigInt(intent.expected_core_au) * TOKEN_SCALE, rateAu) + tapDust(intent.id);
  // Once submission starts, its uniquely dusted amount is the recovery key.
  // Before broadcast, refresh it from the current canonical rate.
  const amountWei = checkpoint.tap_submission_started_at
    ? BigInt(checkpoint.tap_bridge_amount_wei)
    : calculatedAmountWei;
  checkpoint.tap_bridge_amount_wei = amountWei.toString();
  atomicJson(config.checkpointFile(intent.id), checkpoint);
  const preferred = rpc.selectedIndex ?? 0;
  const rpcOrder = [preferred, ...config.tapRpcUrls.map((_url, index) => index).filter((index) => index !== preferred)];
  let common = null;
  let dry = null;
  let lastDryError = null;
  for (const index of rpcOrder) {
    const candidate = config.tapRpcUrls[index];
    try {
      const chainId = parseHexInt(await rpc.callUrl(candidate, 'eth_chainId', []), 'Ethereum chain id');
      if (chainId !== BigInt(config.tapChainId)) continue;
      const candidateArgs = [
        'pay', 'tap', '--amount-wei', amountWei.toString(), '--home', config.buyerHome,
        '--peer-rpc-url', config.coreRpc, '--wallet-password-file', config.walletPasswordFile,
        '--eth-rpc', candidate, '--json',
      ];
      dry = jsonOutput((await runCommand(config.mayhemBin, candidateArgs, { api: config.api, work })).stdout);
      common = candidateArgs;
      rpc.selected = candidate;
      rpc.selectedIndex = index;
      break;
    } catch (error) {
      lastDryError = error;
    }
  }
  if (!dry || !common) {
    if (lastDryError instanceof ReviewWork || lastDryError instanceof RetryWork) throw lastDryError;
    throw new RetryWork('rpc_unavailable', 30, true);
  }
  let buyer;
  try {
    ({ ethereumAccount: buyer } = validateTapBridgePreflight(dry, {
      platformBuyer: config.platformBuyer,
      collection: intent.destination,
      coreRpc: config.coreRpc,
    }));
  } catch {
    throw new RetryWork('core_unavailable', 60, true);
  }
  const pool = normalizeHex(tap.pool_address, 20, 'TAP pool');
  let deposit = await findTapDeposit(rpc, pool, buyer, amountWei, config.tapBridgeLookbackBlocks);
  if (!deposit) {
    // An Ethereum submission has a small uncertainty window: the signer can
    // broadcast successfully and die before the child returns its hash. Never
    // submit again from that state. Keep scanning for the uniquely dusted
    // amount and surface the intent operationally until the chain resolves it.
    if (checkpoint.tap_submission_started_at && !checkpoint.tap_bridge_tx_hash) {
      throw new RetryWork('bridge_submission_uncertain', 30, true);
    }
    if (checkpoint.tap_bridge_tx_hash) {
      const pendingReceipt = await rpc.call('eth_getTransactionReceipt', [checkpoint.tap_bridge_tx_hash]);
      if (!pendingReceipt) throw new RetryWork('core_bridge_pending', 30, true);
      if (parseHexInt(pendingReceipt.status, 'TAP bridge receipt status') !== 1n) {
        throw new ReviewWork('bridge_transaction_failed');
      }
      const matchingLogs = (Array.isArray(pendingReceipt.logs) ? pendingReceipt.logs : []).filter((log) =>
        String(log?.address).toLowerCase() === pool &&
        String(log?.topics?.[0]).toLowerCase() === TAP_DEPOSIT_EVENT_SIGNATURE &&
        String(log?.topics?.[1]).toLowerCase() === addressTopic(buyer) &&
        parseHexInt(log.data, 'TAP bridge amount') === amountWei);
      if (matchingLogs.length !== 1) throw new ReviewWork('bridge_transaction_mismatch');
      deposit = matchingLogs[0];
    }
  }
  if (!deposit) {
    checkpoint.tap_submission_started_at = new Date().toISOString();
    atomicJson(config.checkpointFile(intent.id), checkpoint);
    const submitted = jsonOutput((await runCommand(config.mayhemBin, [...common.slice(0, -1), '--confirm', '--json'], {
      api: config.api, work,
    })).stdout);
    checkpoint.tap_bridge_tx_hash = normalizeHex(submitted.deposit_tx_hash, 32, 'TAP bridge transaction');
    atomicJson(config.checkpointFile(intent.id), checkpoint);
    const receipt = await rpc.call('eth_getTransactionReceipt', [checkpoint.tap_bridge_tx_hash]);
    const logs = (Array.isArray(receipt?.logs) ? receipt.logs : []).filter((log) =>
      String(log?.address).toLowerCase() === pool &&
      String(log?.topics?.[0]).toLowerCase() === TAP_DEPOSIT_EVENT_SIGNATURE &&
      String(log?.topics?.[1]).toLowerCase() === addressTopic(buyer) &&
      parseHexInt(log.data, 'TAP bridge amount') === amountWei);
    if (logs.length !== 1) throw new ReviewWork('bridge_transaction_mismatch');
    deposit = logs[0];
  }
  const { finalizedNumber } = await tapFinality(rpc);
  const blockNumber = parseHexInt(deposit.blockNumber, 'TAP bridge block');
  if (blockNumber > finalizedNumber) throw new RetryWork('core_bridge_pending', 20, true);
  const normalized = {
    chain_id: config.tapChainId,
    pool_address: pool,
    eth_tx_hash: normalizeHex(deposit.transactionHash, 32, 'TAP bridge transaction'),
    log_index: Number(parseHexInt(deposit.logIndex, 'TAP bridge log index')),
    block_hash: normalizeHex(deposit.blockHash, 32, 'TAP bridge block hash'),
  };
  const key = `dep/tap/${tapDepositKey(normalized)}`;
  const record = await waitForCoreRecord(config, work, key, intent.expected_core_au, config.bridgeTimeoutSeconds);
  if (String(record.value.who).toLowerCase() !== config.platformBuyer || BigInt(record.value.au) < BigInt(intent.expected_core_au)) {
    throw new RetryWork('core_bridge_pending', 30, true);
  }
  checkpoint.core_record_key = key;
  atomicJson(config.checkpointFile(intent.id), checkpoint);
  return { backed: true, buyer: config.platformBuyer, rail: 'TAP', creditedAu: BigInt(record.value.au),
    confirmedSignedLength: record.signedLength, recordKey: key };
}

async function processWork(config, work) {
  console.log(JSON.stringify({ event: 'crypto_payment_processing', intent_id: work.intent.id, rail: work.intent.rail }));
  const checkpointFile = config.checkpointFile(work.intent.id);
  const checkpoint = readJson(checkpointFile, { schema_version: 1, intent_id: work.intent.id });
  if (checkpoint.intent_id !== work.intent.id) throw new ReviewWork('malformed_transfer');
  let external;
  let rpc = null;
  if (checkpoint.external_evidence) {
    external = reviveEvidence(checkpoint.external_evidence);
  } else if (work.intent.rail === 'TAP') {
    rpc = new TapRpc(config.tapRpcUrls, config.tapChainId);
    await rpc.select();
    external = await verifyTapCustomerTransfer(work.intent, rpc);
  } else {
    external = await verifyTnkCustomerTransfer(work.intent, config);
  }
  checkpoint.external_evidence = serializeEvidence(external);
  atomicJson(checkpointFile, checkpoint);
  console.log(JSON.stringify({ event: 'crypto_payment_external_verified', intent_id: work.intent.id, rail: work.intent.rail }));
  if (work.intent.late_submission) {
    throw new ReviewWork('late_submission', external);
  }
  const core = work.intent.rail === 'TAP'
    ? await bridgeTap(config, work, checkpoint, rpc ?? new TapRpc(config.tapRpcUrls, config.tapChainId))
    : await bridgeTnk(config, work, checkpoint);
  console.log(JSON.stringify({ event: 'crypto_payment_core_backed', intent_id: work.intent.id, rail: work.intent.rail }));
  await config.api.evidence(work, { ...external, core });
  checkpoint.credited = true;
  checkpoint.credited_at = new Date().toISOString();
  atomicJson(checkpointFile, checkpoint);
}

async function processWorkWithLease(config, work) {
  const renew = setInterval(
    () => void config.api.renew(work).catch(() => undefined),
    30_000,
  );
  try {
    await processWork(config, work);
  } finally {
    clearInterval(renew);
  }
}

function reviveEvidence(value) {
  return {
    ...value,
    blockNumber: BigInt(value.blockNumber),
    tokenAmountBaseUnits: BigInt(value.tokenAmountBaseUnits),
    observedAt: new Date(value.observedAt),
  };
}

function configuration(env = process.env) {
  const stateDir = path.resolve(env.OPENMAYHEM_CRYPTO_WORKER_STATE_DIR || '/var/lib/openmayhem/retail-crypto-worker');
  const primary = requiredEnv('MAYHEM_TAP_ETH_RPC', env);
  const fallbacks = String(env.OPENMAYHEM_TAP_ETH_RPC_FALLBACKS ?? '').split(/[;,\s]+/).filter(Boolean);
  const workerId = env.OPENMAYHEM_CRYPTO_WORKER_ID || 'retail-crypto-worker';
  const tnkNetwork = env.MAYHEM_MSB_NETWORK || 'mainnet';
  const api = new WorkerApi(
    requiredEnv('OPENMAYHEM_CRYPTO_API_URL', env),
    requiredEnv('OPENMAYHEM_CRYPTO_WORKER_SECRET', env),
    workerId,
  );
  return {
    api,
    workerId,
    stateDir,
    coreRpc: requiredEnv('MAYHEM_PEER_RPC', env),
    platformBuyer: normalizeHex64(requiredEnv('OPENMAYHEM_CRYPTO_PLATFORM_BUYER', env), 'platform buyer'),
    buyerHome: path.resolve(requiredEnv('OPENMAYHEM_CRYPTO_BUYER_HOME', env)),
    walletPasswordFile: path.resolve(requiredEnv('OPENMAYHEM_CRYPTO_WALLET_PASSWORD_FILE', env)),
    mayhemBin: path.resolve(env.MAYHEM_BIN || path.join(repoRoot, 'target/release/mayhem')),
    tapRpcUrls: [primary, ...fallbacks.filter((url) => url !== primary)],
    tapChainId: positiveInt(env.MAYHEM_TAP_ETH_CHAIN_ID, 1, 'TAP chain id'),
    tapToken: normalizeHex(requiredEnv('OPENMAYHEM_TAP_TOKEN_ADDRESS', env), 20, 'TAP token'),
    tapCollection: normalizeHex(requiredEnv('OPENMAYHEM_TAP_COLLECTION_ADDRESS', env), 20, 'TAP collection'),
    tapSettlementGasAddress: normalizeHex(
      requiredEnv('OPENMAYHEM_TAP_SETTLEMENT_GAS_ADDRESS', env),
      20,
      'TAP settlement gas address',
    ),
    tapMinimumGasWei: BigInt(env.OPENMAYHEM_TAP_MINIMUM_GAS_WEI || '5000000000000000'),
    settlementMaxEpochLag: positiveInt(env.OPENMAYHEM_SETTLEMENT_MAX_EPOCH_LAG, 2, 'settlement max epoch lag'),
    tapBridgeLookbackBlocks: positiveInt(env.OPENMAYHEM_TAP_BRIDGE_LOOKBACK_BLOCKS, 7200, 'TAP bridge lookback'),
    tnkNetwork,
    tnkCollection: normalizeTnkAddress(
      requiredEnv('OPENMAYHEM_TNK_COLLECTION_ADDRESS', env),
      tnkNetwork,
      'TNK collection address',
    ),
    tnkFinality: positiveInt(env.OPENMAYHEM_TNK_FINALITY_SIGNED_LENGTHS, 2, 'TNK finality'),
    tnkLookback: positiveInt(env.OPENMAYHEM_TNK_LOOKBACK_SIGNED_LENGTHS, 5000, 'TNK lookback'),
    tnkReaderStore: env.OPENMAYHEM_TNK_READER_STORE || 'openmayhem-retail-crypto-reader',
    msbStoresDirectory: path.resolve(requiredEnv('OPENMAYHEM_CRYPTO_MSB_STORES_DIRECTORY', env)),
    msbStoreName: requiredEnv('OPENMAYHEM_CRYPTO_MSB_STORE_NAME', env),
    readerTimeoutSeconds: positiveInt(env.OPENMAYHEM_CRYPTO_READER_TIMEOUT_SECONDS, 60, 'reader timeout'),
    bridgeTimeoutSeconds: positiveInt(env.OPENMAYHEM_CRYPTO_BRIDGE_TIMEOUT_SECONDS, 900, 'bridge timeout'),
    intervalSeconds: positiveInt(env.OPENMAYHEM_CRYPTO_WORKER_INTERVAL_SECONDS, 5, 'worker interval'),
    statusIntervalSeconds: positiveInt(env.OPENMAYHEM_CRYPTO_STATUS_INTERVAL_SECONDS, 60, 'status interval'),
    discoveryIntervalSeconds: positiveInt(env.OPENMAYHEM_CRYPTO_DISCOVERY_INTERVAL_SECONDS, 5, 'discovery interval'),
    tnkDiscoveryReaderStore: env.OPENMAYHEM_TNK_DISCOVERY_READER_STORE || 'openmayhem-retail-crypto-discovery',
    checkpointFile: (intentId) => path.join(stateDir, 'intents', `${intentId}.json`),
    discoveryCheckpointFile: (rail) => path.join(stateDir, 'discovery', `${rail}.json`),
  };
}

async function main() {
  const config = configuration();
  fs.mkdirSync(config.stateDir, { recursive: true, mode: 0o700 });
  let nextStatusAt = 0;
  let statusRunning = false;
  let nextDiscoveryAt = 0;
  let discoveryRunning = false;
  while (true) {
    let work = null;
    try {
      if (!statusRunning && Date.now() >= nextStatusAt) {
        nextStatusAt = Date.now() + config.statusIntervalSeconds * 1_000;
        statusRunning = true;
        void reportOperationalStatus(config).finally(() => { statusRunning = false; });
      }
      if (!discoveryRunning && Date.now() >= nextDiscoveryAt) {
        nextDiscoveryAt = Date.now() + config.discoveryIntervalSeconds * 1_000;
        discoveryRunning = true;
        void discoverIncoming(config)
          .catch(() => console.error(JSON.stringify({ event: 'crypto_payment_discovery_unavailable' })))
          .finally(() => { discoveryRunning = false; });
      }
      work = (await config.api.pull())?.work ?? null;
      if (!work) {
        await sleep(config.intervalSeconds * 1_000);
        continue;
      }
      await processWorkWithLease(config, work);
      console.log(JSON.stringify({ event: 'crypto_payment_credited', intent_id: work.intent.id, rail: work.intent.rail }));
    } catch (error) {
      if (work && error instanceof ReviewWork) {
        await config.api.review(work, error.reason, error.evidence).catch(() => undefined);
        console.error(JSON.stringify({ event: 'crypto_payment_review', intent_id: work.intent.id, rail: work.intent.rail, reason: error.reason }));
      } else if (work) {
        const retry = error instanceof RetryWork ? error : new RetryWork('core_unavailable', 30, true);
        await config.api.retry(work, retry).catch(() => undefined);
        console.error(JSON.stringify({ event: 'crypto_payment_retry', intent_id: work.intent.id, rail: work.intent.rail, code: retry.code }));
      } else {
        console.error(JSON.stringify({ event: 'crypto_payment_worker_unavailable' }));
      }
      await sleep(config.intervalSeconds * 1_000);
    }
  }
}

function isDirectExecution(argument) {
  if (!argument) return false;
  try {
    return fs.realpathSync(argument) === fs.realpathSync(scriptPath);
  } catch {
    return path.resolve(argument) === scriptPath;
  }
}

if (isDirectExecution(process.argv[1])) {
  main().catch((error) => {
    console.error(error?.message ?? String(error));
    process.exit(1);
  });
}
