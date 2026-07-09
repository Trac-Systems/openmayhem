#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import { MainSettlementBus } from 'trac-msb/src/index.js';
import {
  getTxDetailsCommand,
  getTxHashesCommand,
} from 'trac-msb/src/utils/cliCommands.js';

import {
  boolArg,
  createLocalConfig,
  defaultStateDir,
  normalizeNetwork,
  parseArgs,
  readLocalNetwork,
  sleep,
} from './msb-local-common.mjs';

const scriptPath = fileURLToPath(import.meta.url);
const DEFAULT_CURSOR = path.resolve('.mayhem-local', 'tnk-deposit-watcher.json');
const MAX_UNMATCHED_TRANSFERS = 1000;

const isHex64 = (value) => /^[0-9a-f]{64}$/i.test(String(value ?? ''));

function shellQuote(value) {
  const raw = String(value ?? '');
  if (raw.length === 0) return "''";
  return `'${raw.replaceAll("'", "'\\''")}'`;
}

function canonicalAmount(value) {
  try {
    const amount = BigInt(String(value ?? '').trim());
    if (amount <= 0n) return null;
    return amount.toString();
  } catch (_error) {
    return null;
  }
}

function positiveSafeInteger(value) {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed > 0 ? parsed : null;
}

function nonNegativeSafeInteger(value) {
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) && parsed >= 0 ? parsed : null;
}

function sameAddress(left, right) {
  return String(left ?? '').trim().toLowerCase() === String(right ?? '').trim().toLowerCase();
}

function ensureRpcBase(url) {
  const raw = String(url ?? '').trim();
  if (!raw) throw new Error('Missing --peer-rpc or --pending-file.');
  return raw.endsWith('/') ? raw : `${raw}/`;
}

async function fetchJson(url, fetchImpl = globalThis.fetch) {
  const response = await fetchImpl(url);
  if (!response.ok) {
    throw new Error(`GET ${url} failed with ${response.status}`);
  }
  return await response.json();
}

export async function readContractStateValue(rpcUrl, key, {
  confirmed = false,
  fetchImpl = globalThis.fetch,
} = {}) {
  const url = new URL('state', ensureRpcBase(rpcUrl));
  url.searchParams.set('key', key);
  url.searchParams.set('confirmed', confirmed ? 'true' : 'false');
  const body = await fetchJson(url, fetchImpl);
  return body?.value ?? null;
}

export async function resolveActiveBillingEpoch(explicitEpoch, rpcUrl, {
  fetchImpl = globalThis.fetch,
} = {}) {
  if (explicitEpoch !== undefined && explicitEpoch !== null && explicitEpoch !== '') {
    const epoch = positiveSafeInteger(explicitEpoch);
    if (epoch === null) throw new Error('--epoch must be a positive safe integer');
    return epoch;
  }
  if (!rpcUrl) throw new Error('Missing --epoch or --peer-rpc for active billing epoch discovery.');
  const applyState = await readContractStateValue(rpcUrl, 'epoch/apply/state', { fetchImpl });
  const updatedEpoch = applyState?.updated_epoch ?? 0;
  if (!Number.isSafeInteger(updatedEpoch) || updatedEpoch < 0) {
    throw new Error('epoch/apply/state.updated_epoch must be a non-negative safe integer');
  }
  const epoch = updatedEpoch + 1;
  if (!Number.isSafeInteger(epoch) || epoch <= 0) {
    throw new Error('active billing epoch overflowed');
  }
  return epoch;
}

function readJsonIfExists(filePath, fallback) {
  if (!filePath || !fs.existsSync(filePath)) return fallback;
  return JSON.parse(fs.readFileSync(filePath, 'utf8'));
}

function writeJson(filePath, value) {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

function normalizeCursor(raw) {
  const cursor = raw && typeof raw === 'object' ? raw : {};
  return {
    next_signed_length: Number.isSafeInteger(cursor.next_signed_length)
      ? cursor.next_signed_length
      : null,
    seen_msb_tx_hashes: Array.isArray(cursor.seen_msb_tx_hashes)
      ? cursor.seen_msb_tx_hashes.filter(isHex64)
      : [],
    unmatched_transfers: Array.isArray(cursor.unmatched_transfers)
      ? cursor.unmatched_transfers.filter((transfer) => isHex64(transfer?.hash))
      : [],
  };
}

export function pendingEntriesFromState(raw) {
  const source = Array.isArray(raw)
    ? raw
    : Array.isArray(raw?.values)
      ? raw.values
      : raw && typeof raw === 'object'
        ? Object.entries(raw).map(([key, value]) => ({ key, value }))
        : [];

  return source
    .map((entry) => {
      const value = entry?.value ?? entry;
      const memo = value?.memo_hash ?? String(entry?.key ?? '').replace(/^dep\/pending\//, '');
      return {
        key: entry?.key ?? `dep/pending/${memo}`,
        value: { ...value, memo_hash: memo },
      };
    })
    .filter((entry) => entry.value?.status === 'pending' && entry.value?.memo_hash);
}

export async function readPendingIntents({ peerRpc, pendingFile }) {
  if (pendingFile) {
    return pendingEntriesFromState(JSON.parse(fs.readFileSync(pendingFile, 'utf8')));
  }
  const url = new URL('state', ensureRpcBase(peerRpc));
  url.searchParams.set('prefix', 'dep/pending/');
  url.searchParams.set('confirmed', 'false');
  url.searchParams.set('limit', '1000');
  const response = await fetchJson(url);
  return pendingEntriesFromState(response);
}

export function pubKeyHexToMsbAddress(pubKeyHex, addressPrefix) {
  if (!isHex64(pubKeyHex)) return null;
  return PeerWallet.encodeBech32mSafe(addressPrefix, b4a.from(pubKeyHex, 'hex'));
}

export function transferFromTxDetails(hashEntry, details) {
  const payload = details?.txDetails ?? details?.payload ?? details;
  const tro = payload?.tro ?? payload?.transfer ?? null;
  if (!payload || !tro) return null;
  const amount = canonicalAmount(tro.am ?? tro.amount ?? tro.tnk_e18);
  if (!amount) return null;
  const hash = String(hashEntry?.hash ?? payload?.hash ?? tro.tx ?? '').toLowerCase();
  if (!isHex64(hash)) return null;
  return {
    hash,
    confirmed_length: Number.isSafeInteger(hashEntry?.confirmed_length)
      ? hashEntry.confirmed_length
      : null,
    from: payload.address ?? payload.from ?? null,
    to: tro.to ?? tro.recipient ?? null,
    tnk_e18: amount,
  };
}

export function matchPendingTransfers({
  pendingEntries,
  transfers,
  treasuryAddress,
  addressPrefix,
  seen = new Set(),
}) {
  const matches = [];
  const skipped = [];
  const used = new Set();
  const orderedPending = pendingEntries
    .map((entry) => entry.value ?? entry)
    .sort((a, b) => String(a.requested_at ?? '').localeCompare(String(b.requested_at ?? '')));

  for (const pending of orderedPending) {
    const memoHash = String(pending.memo_hash ?? '');
    const expectedSender = pubKeyHexToMsbAddress(pending.user, addressPrefix);
    const expectedTreasury = pending.treasury_address ?? treasuryAddress;
    const expectedAmount = canonicalAmount(pending.tnk_e18);
    const quotedAu = canonicalAmount(pending.quoted_au);
    const rateTnkUsdAu = canonicalAmount(pending.rate_tnk_usd_au);
    const rateSource = String(pending.rate_source ?? '').trim();

    if (!expectedSender) {
      skipped.push({ memo_hash: memoHash, reason: 'pending user is not a 32-byte public key' });
      continue;
    }
    if (!expectedTreasury) {
      skipped.push({ memo_hash: memoHash, reason: 'pending intent has no treasury address and watcher has none' });
      continue;
    }
    if (!expectedAmount) {
      skipped.push({ memo_hash: memoHash, reason: 'pending intent has no quoted TNK amount' });
      continue;
    }
    if (!quotedAu) {
      skipped.push({ memo_hash: memoHash, reason: 'pending intent has no quoted au_usd credit' });
      continue;
    }
    if (!rateTnkUsdAu || !rateSource) {
      skipped.push({ memo_hash: memoHash, reason: 'pending intent has no quoted TNK/USD rate evidence' });
      continue;
    }

    const candidates = transfers.filter((transfer) => (
      !seen.has(transfer.hash)
      && !used.has(transfer.hash)
      && sameAddress(transfer.from, expectedSender)
      && sameAddress(transfer.to, expectedTreasury)
      && (!expectedAmount || transfer.tnk_e18 === expectedAmount)
    ));

    if (candidates.length === 0) {
      skipped.push({ memo_hash: memoHash, reason: 'no finalized matching MSB transfer observed' });
      continue;
    }
    if (candidates.length > 1) {
      skipped.push({
        memo_hash: memoHash,
        reason: 'multiple matching MSB transfers observed; manual admin review required',
        msb_tx_hashes: candidates.map((transfer) => transfer.hash),
      });
      continue;
    }

    const transfer = candidates[0];
    used.add(transfer.hash);
    matches.push({
      memo_hash: memoHash,
      user: pending.user,
      from: transfer.from,
      treasury_address: expectedTreasury,
      tnk_e18: transfer.tnk_e18,
      msb_tx_hash: transfer.hash,
      confirmed_length: transfer.confirmed_length,
      quoted_au: quotedAu,
      rate_tnk_usd_au: rateTnkUsdAu,
      rate_source: rateSource,
    });
  }

  return { matches, skipped };
}

export function buildAdminCommandArgs(match, {
  epoch,
  at,
  submit = true,
  sim = false,
  json = true,
  rpcUrl,
  home,
  peerStoreName,
  walletPassword,
} = {}) {
  const args = [
    'admin',
    'tnk-deposit',
    '--memo-hash',
    match.memo_hash,
    '--tnk-e18',
    match.tnk_e18,
    '--msb-tx-hash',
    match.msb_tx_hash,
    '--epoch',
    String(epoch),
    '--at',
    String(at),
  ];
  if (home) args.push('--home', home);
  if (rpcUrl) args.push('--rpc-url', rpcUrl);
  if (peerStoreName) args.push('--peer-store-name', peerStoreName);
  if (walletPassword) args.push('--wallet-password', walletPassword);
  if (submit) args.push('--submit');
  if (sim) args.push('--sim');
  if (json) args.push('--json');
  return args;
}

export function buildAdminCommand(match, options = {}) {
  const bin = options.mayhemBin ?? 'mayhem';
  const args = buildAdminCommandArgs(match, options)
    .filter((arg, index, all) => all[index - 1] !== '--wallet-password' && arg !== '--wallet-password');
  return [bin, ...args].map(shellQuote).join(' ');
}

export function depositStateMatches(match, {
  pending,
  balance,
  depositRoot,
  epoch,
} = {}) {
  const quotedAu = canonicalAmount(match?.quoted_au);
  if (!quotedAu) return false;
  const balanceAu = canonicalAmount(balance?.au);
  const depositCount = nonNegativeSafeInteger(depositRoot?.count);
  const depositAuTotal = canonicalAmount(depositRoot?.au_total);
  return pending === null
    && balance?.user === match.user
    && balance?.rail === 'tnk'
    && balance?.denom === 'au_usd'
    && balanceAu !== null
    && BigInt(balanceAu) >= BigInt(quotedAu)
    && depositRoot?.type === 'deposit_root'
    && Number(depositRoot?.epoch) === Number(epoch)
    && depositCount !== null
    && depositCount > 0
    && depositAuTotal !== null
    && BigInt(depositAuTotal) >= BigInt(quotedAu);
}

export async function waitForDepositState(match, {
  rpcUrl,
  epoch,
  timeoutMs = 30_000,
  pollMs = 500,
  fetchImpl = globalThis.fetch,
} = {}) {
  const deadline = Date.now() + timeoutMs;
  let state = null;
  while (Date.now() <= deadline) {
    const [pending, balance, depositRoot] = await Promise.all([
      readContractStateValue(rpcUrl, `dep/pending/${match.memo_hash}`, { fetchImpl }),
      readContractStateValue(rpcUrl, `bal/${match.user}/tnk`, { fetchImpl }),
      readContractStateValue(rpcUrl, `ev/dep/${epoch}`, { fetchImpl }),
    ]);
    state = { pending, balance, depositRoot };
    if (depositStateMatches(match, { ...state, epoch })) {
      return { verified: true, state };
    }
    await sleep(pollMs);
  }
  return { verified: false, state };
}

async function scanMsbTransfers(msb, {
  fromSignedLength,
  finalitySignedLengths,
  chunkSize,
  timeoutSec,
}) {
  let confirmedLength = msb.state.getSignedLength();
  for (let waited = 0; confirmedLength === 0 && waited < timeoutSec; waited += 1) {
    await sleep(1000);
    confirmedLength = msb.state.getSignedLength();
  }
  const safeEnd = Math.max(0, confirmedLength - finalitySignedLengths);
  if (safeEnd <= fromSignedLength) {
    return { confirmedLength, safeEnd, transfers: [] };
  }

  const transfers = [];
  for (let start = fromSignedLength; start < safeEnd; start += chunkSize) {
    const end = Math.min(start + chunkSize, safeEnd);
    const { hashes } = await getTxHashesCommand(msb.state, start, end);
    for (const hashEntry of hashes) {
      const details = await getTxDetailsCommand(msb.state, hashEntry.hash, msb.config);
      const transfer = transferFromTxDetails(hashEntry, details);
      if (transfer) transfers.push(transfer);
    }
  }
  return { confirmedLength, safeEnd, transfers };
}

function mergeTransfers(existing, incoming, treasuryAddress) {
  const merged = new Map();
  for (const transfer of [...existing, ...incoming]) {
    if (!isHex64(transfer?.hash)) continue;
    if (treasuryAddress && !sameAddress(transfer.to, treasuryAddress)) continue;
    merged.set(transfer.hash, transfer);
  }
  return Array.from(merged.values())
    .sort((a, b) => (
      Number(a.confirmed_length ?? 0) - Number(b.confirmed_length ?? 0)
      || String(a.hash).localeCompare(String(b.hash))
    ))
    .slice(-MAX_UNMATCHED_TRANSFERS);
}

function parsePositiveInt(value, label, fallback = null) {
  if (value === undefined || value === null || value === '') {
    if (fallback !== null) return fallback;
    throw new Error(`Missing ${label}`);
  }
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 0) {
    throw new Error(`${label} must be a non-negative safe integer`);
  }
  return parsed;
}

async function main() {
  const args = parseArgs();
  const stateDir = path.resolve(args['state-dir'] || defaultStateDir());
  const useLocal = boolArg(args.local, false);
  const net = useLocal ? readLocalNetwork(stateDir) : null;
  const network = normalizeNetwork(args.network || net?.network || process.env.MAYHEM_MSB_NETWORK || 'testnet1');
  const treasuryAddress = String(args['treasury-address'] || process.env.MAYHEM_TNK_TREASURY_ADDRESS || '').trim();
  if (!treasuryAddress) throw new Error('Missing --treasury-address.');

  const adminRpcUrl = args['admin-rpc-url'] || args['peer-rpc'] || process.env.MAYHEM_PEER_RPC;
  const epoch = await resolveActiveBillingEpoch(args.epoch, adminRpcUrl);
  const at = parsePositiveInt(args.at ?? Math.floor(Date.now() / 1000), '--at');
  const timeoutSec = parsePositiveInt(args.timeout ?? 30, '--timeout', 30);
  const finalitySignedLengths = parsePositiveInt(
    args['finality-signed-lengths'] ?? process.env.MAYHEM_TNK_FINALITY_SIGNED_LENGTHS ?? 0,
    '--finality-signed-lengths',
    0
  );
  const chunkSize = Math.max(1, parsePositiveInt(args['chunk-size'] ?? 500, '--chunk-size', 500));
  const lookback = parsePositiveInt(args.lookback ?? 5000, '--lookback', 5000);
  const cursorPath = path.resolve(args.cursor || DEFAULT_CURSOR);
  const cursor = normalizeCursor(readJsonIfExists(cursorPath, {}));
  const json = boolArg(args.json, false);
  const submit = boolArg(args.submit, false);
  const sim = boolArg(args.sim, false);
  const verify = !boolArg(args['no-verify'], false);
  const verifyTimeoutMs = parsePositiveInt(args['verify-timeout-ms'] ?? 30_000, '--verify-timeout-ms', 30_000);
  const verifyPollMs = parsePositiveInt(args['verify-poll-ms'] ?? 500, '--verify-poll-ms', 500);

  const config = createLocalConfig({
    network,
    stateDir: path.join(stateDir, 'deposit-watchers'),
    storeName: String(args['store-name'] || `deposit-watcher-${network}`),
    channel: args['msb-channel'] || net?.channel || process.env.MSB_CHANNEL || undefined,
    bootstrap: args['msb-bootstrap'] || net?.bootstrap || process.env.MSB_BOOTSTRAP || undefined,
    enableWallet: false,
    dhtBootstrap: net?.dht_bootstrap || undefined,
  });

  const msb = new MainSettlementBus(config);
  await msb.ready();

  let scan;
  let fromSignedLength;
  try {
    const confirmedLength = msb.state.getSignedLength();
    fromSignedLength = args['from-signed-length'] !== undefined
      ? parsePositiveInt(args['from-signed-length'], '--from-signed-length')
      : cursor.next_signed_length ?? Math.max(0, confirmedLength - lookback);
    scan = await scanMsbTransfers(msb, {
      fromSignedLength,
      finalitySignedLengths,
      chunkSize,
      timeoutSec,
    });
  } finally {
    try {
      await msb.close();
    } catch (_error) {}
  }

  const pendingEntries = await readPendingIntents({
    peerRpc: adminRpcUrl,
    pendingFile: args['pending-file'],
  });

  const seen = new Set(cursor.seen_msb_tx_hashes);
  const unmatchedTransfers = mergeTransfers(cursor.unmatched_transfers, scan.transfers, treasuryAddress)
    .filter((transfer) => !seen.has(transfer.hash));
  const { matches, skipped } = matchPendingTransfers({
    pendingEntries,
    transfers: unmatchedTransfers,
    treasuryAddress,
    addressPrefix: config.addressPrefix,
    seen,
  });

  const adminOptions = {
    epoch,
    at,
    submit: true,
    sim,
    json: true,
    mayhemBin: args['mayhem-bin'] || 'mayhem',
    rpcUrl: adminRpcUrl,
    home: args['admin-home'],
    peerStoreName: args['admin-peer-store-name'],
    walletPassword: args['admin-wallet-password-env']
      ? process.env[String(args['admin-wallet-password-env'])]
      : undefined,
  };
  const confirmations = [];
  const submittedHashes = new Set();
  for (const match of matches) {
    const command = buildAdminCommand(match, adminOptions);
    const confirmation = {
      ...match,
      copy_paste_admin_submit_command: command,
      submitted: false,
      preflighted: false,
      verified: false,
    };
    if (submit) {
      const child = spawnSync(
        args['mayhem-bin'] || 'mayhem',
        buildAdminCommandArgs(match, adminOptions),
        { encoding: 'utf8' }
      );
      if (sim) {
        confirmation.preflighted = child.status === 0;
      } else {
        confirmation.submitted = child.status === 0;
      }
      confirmation.exit_status = child.status;
      confirmation.stdout = child.stdout?.trim() || null;
      confirmation.stderr = child.stderr?.trim() || null;
      if (child.status !== 0) {
        confirmation.error = `mayhem admin tnk-deposit exited ${child.status}`;
      } else if (!sim) {
        if (adminOptions.rpcUrl && verify) {
          const verification = await waitForDepositState(match, {
            rpcUrl: adminOptions.rpcUrl,
            epoch,
            timeoutMs: verifyTimeoutMs,
            pollMs: verifyPollMs,
          });
          confirmation.verified = verification.verified;
          confirmation.state = verification.state;
          if (verification.verified) {
            submittedHashes.add(match.msb_tx_hash);
          } else {
            confirmation.error = 'tnkDeposit submit did not update pending intent, balance, and deposit root';
          }
        } else {
          confirmation.verification_skipped = true;
          submittedHashes.add(match.msb_tx_hash);
        }
      }
    }
    confirmations.push(confirmation);
  }

  for (const hash of submittedHashes) seen.add(hash);
  const matchedButUnsubmitted = submit ? [] : matches.map((match) => match.msb_tx_hash);
  const nextCursor = {
    next_signed_length: Math.max(fromSignedLength, scan.safeEnd),
    seen_msb_tx_hashes: Array.from(seen).sort(),
    unmatched_transfers: unmatchedTransfers.filter((transfer) => (
      !seen.has(transfer.hash) || matchedButUnsubmitted.includes(transfer.hash)
    )),
  };
  writeJson(cursorPath, nextCursor);

  const report = {
    ok: confirmations.every((item) => item.error === undefined),
    network,
    address_prefix: config.addressPrefix,
    treasury_address: treasuryAddress,
    epoch,
    at,
    finality_signed_lengths: finalitySignedLengths,
    from_signed_length: fromSignedLength,
    safe_end_signed_length: scan.safeEnd,
    current_confirmed_length: scan.confirmedLength,
    cursor: cursorPath,
    pending_count: pendingEntries.length,
    observed_transfer_count: scan.transfers.length,
    unmatched_transfer_count: nextCursor.unmatched_transfers.length,
    confirmations,
    skipped,
  };

  if (json) {
    console.log(JSON.stringify(report, null, 2));
  } else {
    console.log('[tnk:watch] TNK deposit watcher scan complete');
    console.log('[tnk:watch] network:', network);
    console.log('[tnk:watch] treasury:', treasuryAddress);
    console.log('[tnk:watch] confirmed length:', scan.confirmedLength);
    console.log('[tnk:watch] matched confirmations:', confirmations.length);
    for (const item of confirmations) {
      console.log('Copy/paste admin TNK deposit submit command:');
      console.log(item.copy_paste_admin_submit_command);
    }
    if (skipped.length > 0) {
      console.log('[tnk:watch] skipped pending intents:', skipped.length);
    }
  }

  if (!report.ok) process.exit(2);
}

if (process.argv[1] && path.resolve(process.argv[1]) === scriptPath) {
  main().catch((error) => {
    console.error(error?.stack || error?.message || String(error));
    process.exit(1);
  });
}
