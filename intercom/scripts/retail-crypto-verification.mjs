export const ERC20_TRANSFER_TOPIC = '0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef';

export function uniqueIntentByAmount(intents, amountBaseUnits) {
  const amount = String(amountBaseUnits);
  const matches = intents.filter((intent) => String(intent?.token_amount_base_units) === amount);
  return matches.length === 1 ? matches[0] : null;
}

export class RetryWork extends Error {
  constructor(code, delaySeconds, transferObserved = false) {
    super(code);
    this.code = code;
    this.delaySeconds = delaySeconds;
    this.transferObserved = transferObserved;
  }
}

export class ReviewWork extends Error {
  constructor(reason, evidence = null) {
    super(reason);
    this.reason = reason;
    this.evidence = evidence;
  }
}

export function normalizeHex(value, bytes, label) {
  const text = String(value ?? '').trim().toLowerCase();
  if (!new RegExp(`^0x[0-9a-f]{${bytes * 2}}$`).test(text)) throw new Error(`${label} is invalid`);
  return text;
}

export function normalizeHex64(value, label) {
  const text = String(value ?? '').trim().replace(/^0x/i, '').toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(text)) throw new Error(`${label} is invalid`);
  return text;
}

export function normalizeTnkAddress(value, network, label) {
  const text = String(value ?? '').trim().toLowerCase();
  const prefix = network === 'mainnet' ? 'trac1' : network === 'testnet1' ? 'testtrac1' : null;
  if (!prefix || !text.startsWith(prefix) || !/^[0-9a-z]{24,110}$/.test(text)) {
    throw new Error(`${label} is invalid for ${network}`);
  }
  return text;
}

export function validateTapBridgePreflight(report, { platformBuyer, collection, coreRpc }) {
  const ethereumAccount = normalizeHex(report?.from, 20, 'TAP bridge account');
  const boundUser = normalizeHex64(report?.tap_account_binding?.user, 'TAP-bound platform buyer');
  if (ethereumAccount !== normalizeHex(collection, 20, 'TAP collection account') ||
      boundUser !== normalizeHex64(platformBuyer, 'platform buyer') ||
      String(report?.payment_config?.peer_rpc_url) !== coreRpc) {
    throw new Error('TAP bridge preflight does not match the configured buyer');
  }
  return { ethereumAccount, boundUser };
}

export function parseHexInt(value, label) {
  if (typeof value !== 'string' || !/^0x[0-9a-f]+$/i.test(value)) throw new Error(`${label} is invalid`);
  return BigInt(value);
}

export function addressTopic(address) {
  return `0x${'0'.repeat(24)}${normalizeHex(address, 20, 'address').slice(2)}`;
}

export function summarizePayoutLiabilities(records, rail) {
  let totalAu = 0n;
  let heldAu = 0n;
  let paidAu = 0n;
  for (const entry of records) {
    const value = entry?.value;
    if (value?.type !== 'provider_payout_liability' || value.rail !== rail.toLowerCase()) {
      throw new Error(`Invalid ${rail} payout liability record`);
    }
    const total = decimalInteger(value.total_au, 'total_au');
    const held = decimalInteger(value.held_au, 'held_au');
    const paid = decimalInteger(value.paid_cum_au, 'paid_cum_au');
    if (held > total || paid > total || held + paid > total) {
      throw new Error(`Invalid ${rail} payout liability totals`);
    }
    totalAu += total;
    heldAu += held;
    paidAu += paid;
  }
  return {
    totalAu,
    heldAu,
    paidAu,
    unsettledAu: totalAu - paidAu,
    payableAu: totalAu - heldAu - paidAu,
  };
}

function decimalInteger(value, label) {
  const text = String(value ?? '');
  if (!/^(0|[1-9][0-9]*)$/.test(text)) throw new Error(`${label} is invalid`);
  return BigInt(text);
}

function topicAddress(topic, label) {
  const normalized = normalizeHex(topic, 32, label);
  return `0x${normalized.slice(-40)}`;
}

export function verifyTapTransferReceipt(receipt, {
  transactionHash,
  token,
  destination,
  amountBaseUnits,
  latestBlock,
  finalizedBlock,
}) {
  const txHash = normalizeHex(transactionHash, 32, 'transaction hash');
  if (!receipt) throw new RetryWork('transfer_pending', 15);
  if (normalizeHex(receipt.transactionHash, 32, 'receipt transaction hash') !== txHash ||
      parseHexInt(receipt.status, 'receipt status') !== 1n) {
    throw new ReviewWork('malformed_transfer');
  }
  const tokenAddress = normalizeHex(token, 20, 'token');
  const targetTopic = addressTopic(destination);
  const tokenLogs = (Array.isArray(receipt.logs) ? receipt.logs : []).filter((log) =>
    String(log?.address ?? '').toLowerCase() === tokenAddress &&
    String(log?.topics?.[0] ?? '').toLowerCase() === ERC20_TRANSFER_TOPIC);
  const destinationLogs = tokenLogs.filter((log) => String(log?.topics?.[2] ?? '').toLowerCase() === targetTopic);
  const amount = BigInt(String(amountBaseUnits));
  const blockNumber = parseHexInt(receipt.blockNumber, 'receipt block number');
  const current = BigInt(latestBlock);
  const finalized = BigInt(finalizedBlock);
  if (blockNumber > finalized) throw new RetryWork('awaiting_finality', 20, true);
  const exact = destinationLogs.find((log) => parseHexInt(log.data, 'transfer amount') === amount);
  if (!exact) {
    if (destinationLogs.length > 0) {
      throw new ReviewWork('amount_mismatch', tapTransferEvidence(receipt, destinationLogs[0], {
        txHash,
        latestBlock,
        finalizedBlock,
      }));
    }
    if (tokenLogs.length > 0) {
      throw new ReviewWork('wrong_destination', tapTransferEvidence(receipt, tokenLogs[0], {
        txHash,
        latestBlock,
        finalizedBlock,
      }));
    }
    throw new ReviewWork('wrong_token');
  }
  const logIndex = Number(parseHexInt(exact.logIndex, 'transfer log index'));
  if (!Number.isSafeInteger(logIndex)) throw new ReviewWork('malformed_transfer');
  return {
    rail: 'TAP',
    transactionHash: txHash,
    logIndex,
    blockNumber,
    blockHash: normalizeHex(receipt.blockHash, 32, 'receipt block hash'),
    fromAddress: topicAddress(exact.topics[1], 'transfer sender'),
    toAddress: normalizeHex(destination, 20, 'destination'),
    tokenAmountBaseUnits: amount,
    confirmations: Number(current - blockNumber + 1n),
    finalized: true,
    observedAt: new Date(),
    externalRecordKey: `tap/${txHash}/${logIndex}`,
  };
}

function tapTransferEvidence(receipt, log, { txHash, latestBlock, finalizedBlock }) {
  try {
    const blockNumber = parseHexInt(receipt.blockNumber, 'receipt block number');
    const current = BigInt(latestBlock);
    const finalized = BigInt(finalizedBlock);
    const logIndex = Number(parseHexInt(log.logIndex, 'transfer log index'));
    if (!Number.isSafeInteger(logIndex)) throw new Error('invalid log index');
    return {
      rail: 'TAP',
      transactionHash: txHash,
      logIndex,
      blockNumber,
      blockHash: normalizeHex(receipt.blockHash, 32, 'receipt block hash'),
      fromAddress: topicAddress(log.topics[1], 'transfer sender'),
      toAddress: topicAddress(log.topics[2], 'transfer destination'),
      tokenAmountBaseUnits: parseHexInt(log.data, 'transfer amount'),
      confirmations: Number(current >= blockNumber ? current - blockNumber + 1n : 0n),
      finalized: blockNumber <= finalized,
      observedAt: new Date(),
      externalRecordKey: `tap/${txHash}/${logIndex}`,
    };
  } catch {
    throw new ReviewWork('malformed_transfer');
  }
}
