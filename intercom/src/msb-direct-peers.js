const MAX_DIRECT_PEERS = 16;
const PUBLIC_KEY_PATTERN = /^[0-9a-fA-F]{64}$/;

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

export function parseCanonicalMsbDirectPeers(value) {
  if (value === null || value === undefined) return [];
  if (typeof value === 'string' && value.trim() === '') return [];

  const entries = Array.isArray(value)
    ? [...value]
    : typeof value === 'string'
      ? value.split(',')
      : null;
  if (entries === null || entries.length > MAX_DIRECT_PEERS) {
    throw new Error(
      'MSB direct peers must be a list of at most 16 32-byte hexadecimal public keys.'
    );
  }

  const peers = [];
  const seen = new Set();
  for (const entry of entries) {
    if (typeof entry !== 'string' || !PUBLIC_KEY_PATTERN.test(entry.trim())) {
      throw new Error(
        'MSB direct peers must be a list of at most 16 32-byte hexadecimal public keys.'
      );
    }
    const peer = entry.trim().toLowerCase();
    if (seen.has(peer)) continue;
    seen.add(peer);
    peers.push(peer);
  }
  return peers;
}

export async function openMsbWithDirectPeers(msb, {
  directPeers = [],
  timeoutSeconds,
  pollIntervalMs = 10,
  sleepFn = sleep,
  setTimeoutFn = setTimeout,
  clearTimeoutFn = clearTimeout,
} = {}) {
  const peers = parseCanonicalMsbDirectPeers(directPeers);
  if (!Number.isSafeInteger(timeoutSeconds) || timeoutSeconds <= 0) {
    throw new Error('MSB opening timeout must be a positive safe integer.');
  }

  let cancelled = false;
  let deadlineTimer;
  const readyPromise = Promise.resolve().then(() => msb.ready());
  const connectPromise = peers.length === 0
    ? Promise.resolve()
    : (async () => {
        while (!cancelled) {
          const network = msb.network;
          if (network?.swarm) {
            if (typeof network.tryConnect !== 'function') {
              throw new Error('MSB network does not support direct peer connections.');
            }
            for (const peer of peers) {
              const manager = network.validatorConnectionManager;
              if (
                typeof manager?.connected === 'function'
                && manager.connected(peer)
              ) {
                continue;
              }
              await network.tryConnect(peer, 'validator');
            }
            return;
          }
          await sleepFn(pollIntervalMs);
        }
      })();
  const deadline = new Promise((_, reject) => {
    deadlineTimer = setTimeoutFn(() => {
      reject(
        new Error(
          `MSB opening/direct-peer connection timed out after ${timeoutSeconds} seconds.`
        )
      );
    }, timeoutSeconds * 1_000);
  });

  try {
    await Promise.race([
      Promise.all([readyPromise, connectPromise]),
      deadline,
    ]);
  } finally {
    cancelled = true;
    clearTimeoutFn(deadlineTimer);
  }
}
