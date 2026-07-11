import b4a from 'b4a';
import c from '../node_modules/compact-encoding/index.js';

const stringifyJson = (value, label) => {
  const encoded = JSON.stringify(value);
  if (typeof encoded !== 'string') throw new Error(`${label} is not JSON serializable.`);
  return encoded;
};

const boundedJsonEncoding = (maxBytes, label = 'JSON message') => {
  if (!Number.isSafeInteger(maxBytes) || maxBytes <= 0) {
    throw new Error(`${label} maximum byte length must be a positive safe integer.`);
  }

  const checkedJson = (value) => {
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
        return null;
      }
      const encoded = b4a.toString(state.buffer, 'utf8', state.start, end);
      state.start = end;
      try {
        return JSON.parse(encoded);
      } catch (_error) {
        return null;
      }
    },
  };
};

export { boundedJsonEncoding };
