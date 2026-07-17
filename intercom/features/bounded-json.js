import b4a from 'b4a';
import c from '../node_modules/compact-encoding/index.js';

const DEFAULT_MAX_JSON_DEPTH = 256;
const DEFAULT_MAX_JSON_NODES = 100_000;
const DEFAULT_MAX_JSON_STRING_BYTES = 256 * 1024;
const decodedMetadata = new WeakMap();

const stringifyJson = (value, label) => {
  const encoded = JSON.stringify(value);
  if (typeof encoded !== 'string') throw new Error(`${label} is not JSON serializable.`);
  return encoded;
};

const jsonShapeWithinBounds = (
  value,
  maxDepth = DEFAULT_MAX_JSON_DEPTH,
  maxNodes = DEFAULT_MAX_JSON_NODES,
  maxStringBytes = DEFAULT_MAX_JSON_STRING_BYTES
) => {
  if (typeof value === 'string') return b4a.byteLength(value, 'utf8') <= maxStringBytes;
  if (!value || typeof value !== 'object') return true;
  const stack = [{ value, depth: 1 }];
  let nodes = 0;
  while (stack.length > 0) {
    const current = stack.pop();
    nodes += 1;
    if (nodes > maxNodes || current.depth > maxDepth) return false;
    const entries = Array.isArray(current.value)
      ? current.value.map((child) => [null, child])
      : Object.entries(current.value);
    for (const [key, child] of entries) {
      if (key !== null && b4a.byteLength(key, 'utf8') > maxStringBytes) return false;
      if (typeof child === 'string' && b4a.byteLength(child, 'utf8') > maxStringBytes) {
        return false;
      }
    }
    const children = entries.map(([, child]) => child);
    for (const child of children) {
      if (child && typeof child === 'object') {
        stack.push({ value: child, depth: current.depth + 1 });
      }
    }
  }
  return true;
};

const rejectedJson = (bytes, reason) => {
  const value = {};
  decodedMetadata.set(value, { bytes, rejected: true, reason });
  return value;
};

const decodedJsonByteLength = (value) => (
  value && typeof value === 'object' ? decodedMetadata.get(value)?.bytes ?? null : null
);

const decodedJsonWasRejected = (value) => (
  value && typeof value === 'object' && decodedMetadata.get(value)?.rejected === true
);

const boundedJsonEncoding = (maxBytes, label = 'JSON message', options = {}) => {
  if (!Number.isSafeInteger(maxBytes) || maxBytes <= 0) {
    throw new Error(`${label} maximum byte length must be a positive safe integer.`);
  }
  const configuredMaxStringBytes = options?.maxStringBytes;
  const maxStringBytes = Math.min(
    maxBytes,
    Number.isSafeInteger(configuredMaxStringBytes) && configuredMaxStringBytes > 0
      ? configuredMaxStringBytes
      : DEFAULT_MAX_JSON_STRING_BYTES
  );

  const checkedJson = (value) => {
    if (!jsonShapeWithinBounds(value, undefined, undefined, maxStringBytes)) {
      throw new Error(`${label} exceeds JSON shape or string bounds.`);
    }
    const encoded = stringifyJson(value, label);
    const bytes = b4a.byteLength(encoded, 'utf8');
    if (bytes > maxBytes) throw new Error(`${label} exceeds ${maxBytes} bytes.`);
    return encoded;
  };

  return {
    preencode(state, value) {
      c.utf8.preencode(state, checkedJson(value));
    },
    encode(state, value) {
      c.utf8.encode(state, checkedJson(value));
    },
    decode(state) {
      const length = c.uint.decode(state);
      if (!Number.isSafeInteger(length) || length < 0) throw new Error(`${label} length is invalid.`);
      if (state.end - state.start < length) throw new Error(`${label} is truncated.`);
      const end = state.start + length;
      if (length > maxBytes) {
        state.start = end;
        return rejectedJson(length, 'oversized');
      }
      const encoded = b4a.toString(state.buffer, 'utf8', state.start, end);
      state.start = end;
      try {
        const value = JSON.parse(encoded);
        if (!jsonShapeWithinBounds(value, undefined, undefined, maxStringBytes)) {
          return rejectedJson(length, 'complexity');
        }
        if (value && typeof value === 'object') {
          decodedMetadata.set(value, { bytes: length, rejected: false, reason: null });
        }
        return value;
      } catch (_error) {
        return rejectedJson(length, 'malformed');
      }
    },
  };
};

export {
  DEFAULT_MAX_JSON_STRING_BYTES,
  boundedJsonEncoding,
  decodedJsonByteLength,
  decodedJsonWasRejected,
  jsonShapeWithinBounds,
};
