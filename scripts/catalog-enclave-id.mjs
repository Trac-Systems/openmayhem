// The workspace WASM binding exposes unkeyed hashing only; canonical IDs require BLAKE3 derive-key mode.
const IV = Uint32Array.from([
  0x6a09e667,
  0xbb67ae85,
  0x3c6ef372,
  0xa54ff53a,
  0x510e527f,
  0x9b05688c,
  0x1f83d9ab,
  0x5be0cd19,
]);
const MESSAGE_PERMUTATION = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];
const CHUNK_LEN = 1024;
const BLOCK_LEN = 64;
const CHUNK_START = 1;
const CHUNK_END = 2;
const PARENT = 4;
const ROOT = 8;
const DERIVE_KEY_CONTEXT = 32;
const DERIVE_KEY_MATERIAL = 64;

export const CATALOG_ENCLAVE_ID_DOMAIN = 'mayhem-catalog-enclave-id-v2';

function rotateRight(value, bits) {
  return ((value >>> bits) | (value << (32 - bits))) >>> 0;
}

function mix(state, a, b, c, d, left, right) {
  state[a] = (state[a] + state[b] + left) >>> 0;
  state[d] = rotateRight(state[d] ^ state[a], 16);
  state[c] = (state[c] + state[d]) >>> 0;
  state[b] = rotateRight(state[b] ^ state[c], 12);
  state[a] = (state[a] + state[b] + right) >>> 0;
  state[d] = rotateRight(state[d] ^ state[a], 8);
  state[c] = (state[c] + state[d]) >>> 0;
  state[b] = rotateRight(state[b] ^ state[c], 7);
}

function round(state, message) {
  mix(state, 0, 4, 8, 12, message[0], message[1]);
  mix(state, 1, 5, 9, 13, message[2], message[3]);
  mix(state, 2, 6, 10, 14, message[4], message[5]);
  mix(state, 3, 7, 11, 15, message[6], message[7]);
  mix(state, 0, 5, 10, 15, message[8], message[9]);
  mix(state, 1, 6, 11, 12, message[10], message[11]);
  mix(state, 2, 7, 8, 13, message[12], message[13]);
  mix(state, 3, 4, 9, 14, message[14], message[15]);
}

function compress(chainingValue, blockWords, counter, blockLength, flags) {
  const state = new Uint32Array(16);
  state.set(chainingValue, 0);
  state.set(IV.subarray(0, 4), 8);
  state[12] = Number(counter & 0xffffffffn);
  state[13] = Number((counter >> 32n) & 0xffffffffn);
  state[14] = blockLength;
  state[15] = flags;

  let message = Uint32Array.from(blockWords);
  for (let index = 0; index < 7; index += 1) {
    round(state, message);
    message = Uint32Array.from(MESSAGE_PERMUTATION, (position) => message[position]);
  }

  const output = new Uint32Array(8);
  for (let index = 0; index < output.length; index += 1) {
    output[index] = (state[index] ^ state[index + 8]) >>> 0;
  }
  return output;
}

function blockWords(bytes) {
  const block = Buffer.alloc(BLOCK_LEN);
  Buffer.from(bytes).copy(block);
  const words = new Uint32Array(16);
  for (let index = 0; index < words.length; index += 1) {
    words[index] = block.readUInt32LE(index * 4);
  }
  return words;
}

function outputChainingValue(output) {
  return compress(
    output.inputChainingValue,
    output.blockWords,
    output.counter,
    output.blockLength,
    output.flags,
  );
}

function outputRootHash(output) {
  const words = compress(
    output.inputChainingValue,
    output.blockWords,
    0n,
    output.blockLength,
    output.flags | ROOT,
  );
  const digest = Buffer.alloc(32);
  for (let index = 0; index < words.length; index += 1) {
    digest.writeUInt32LE(words[index], index * 4);
  }
  return digest;
}

function chunkOutput(bytes, chunkCounter, keyWords, flags) {
  const blockCount = Math.max(1, Math.ceil(bytes.length / BLOCK_LEN));
  let chainingValue = Uint32Array.from(keyWords);
  for (let index = 0; index < blockCount; index += 1) {
    const start = index * BLOCK_LEN;
    const block = bytes.subarray(start, Math.min(start + BLOCK_LEN, bytes.length));
    const blockFlags =
      flags |
      (index === 0 ? CHUNK_START : 0) |
      (index === blockCount - 1 ? CHUNK_END : 0);
    const output = {
      inputChainingValue: chainingValue,
      blockWords: blockWords(block),
      counter: chunkCounter,
      blockLength: block.length,
      flags: blockFlags,
    };
    if (index === blockCount - 1) return output;
    chainingValue = outputChainingValue(output);
  }
  throw new Error('BLAKE3 chunk output was not produced');
}

function parentOutput(left, right, keyWords, flags) {
  return {
    inputChainingValue: Uint32Array.from(keyWords),
    blockWords: Uint32Array.from([...left, ...right]),
    counter: 0n,
    blockLength: BLOCK_LEN,
    flags: flags | PARENT,
  };
}

function addChunkChainingValue(stack, chainingValue, totalChunks, keyWords, flags) {
  let value = chainingValue;
  let count = totalChunks;
  while ((count & 1n) === 0n) {
    const left = stack.pop();
    value = outputChainingValue(parentOutput(left, value, keyWords, flags));
    count >>= 1n;
  }
  stack.push(value);
}

function blake3Hash(bytes, keyWords, flags) {
  const input = Buffer.from(bytes);
  const chunkCount = Math.max(1, Math.ceil(input.length / CHUNK_LEN));
  const stack = [];

  for (let index = 0; index < chunkCount - 1; index += 1) {
    const start = index * CHUNK_LEN;
    const output = chunkOutput(
      input.subarray(start, start + CHUNK_LEN),
      BigInt(index),
      keyWords,
      flags,
    );
    addChunkChainingValue(
      stack,
      outputChainingValue(output),
      BigInt(index + 1),
      keyWords,
      flags,
    );
  }

  const lastIndex = chunkCount - 1;
  let output = chunkOutput(
    input.subarray(lastIndex * CHUNK_LEN),
    BigInt(lastIndex),
    keyWords,
    flags,
  );
  while (stack.length > 0) {
    output = parentOutput(stack.pop(), outputChainingValue(output), keyWords, flags);
  }
  return outputRootHash(output);
}

function wordsFromKey(key) {
  const words = new Uint32Array(8);
  for (let index = 0; index < words.length; index += 1) {
    words[index] = key.readUInt32LE(index * 4);
  }
  return words;
}

function blake3DeriveKey(context, material) {
  const contextKey = blake3Hash(Buffer.from(context, 'utf8'), IV, DERIVE_KEY_CONTEXT);
  return blake3Hash(material, wordsFromKey(contextKey), DERIVE_KEY_MATERIAL);
}

function requireString(value, name) {
  if (typeof value !== 'string') throw new TypeError(`${name} must be a string`);
  return Buffer.from(value, 'utf8');
}

function u64be(value) {
  const bytes = Buffer.alloc(8);
  bytes.writeBigUInt64BE(BigInt(value));
  return bytes;
}

function lengthPrefixed(value, name) {
  const bytes = requireString(value, name);
  return Buffer.concat([u64be(bytes.length), bytes]);
}

export function orderedSidecarRootEntries(artifactSidecarRoots = {}) {
  if (
    artifactSidecarRoots === null ||
    typeof artifactSidecarRoots !== 'object' ||
    Array.isArray(artifactSidecarRoots)
  ) {
    throw new TypeError('artifactSidecarRoots must be an object');
  }
  return Object.entries(artifactSidecarRoots)
    .map(([name, root]) => {
      requireString(root, `artifactSidecarRoots.${name}`);
      return [name, root];
    })
    .sort(([left], [right]) => Buffer.compare(Buffer.from(left), Buffer.from(right)));
}

export function catalogEnclaveId({
  adminPubkey,
  modelId,
  artifactRoot,
  artifactSidecarRoots = {},
  manifestHash,
}) {
  const sidecars = orderedSidecarRootEntries(artifactSidecarRoots);
  const material = [
    lengthPrefixed(adminPubkey, 'adminPubkey'),
    lengthPrefixed(modelId, 'modelId'),
    lengthPrefixed(artifactRoot, 'artifactRoot'),
    u64be(sidecars.length),
  ];
  for (const [name, root] of sidecars) {
    material.push(lengthPrefixed(name, 'sidecar name'));
    material.push(lengthPrefixed(root, `artifactSidecarRoots.${name}`));
  }
  material.push(lengthPrefixed(manifestHash, 'manifestHash'));
  return blake3DeriveKey(CATALOG_ENCLAVE_ID_DOMAIN, Buffer.concat(material)).toString('hex');
}
