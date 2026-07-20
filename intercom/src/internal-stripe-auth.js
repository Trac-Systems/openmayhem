import b4a from 'b4a';
import Sha256 from 'sha256-universal';
import sodium from 'sodium-universal';

const sha256Hex = (value) => {
  const digest = b4a.alloc(Sha256.SHA256_BYTES);
  Sha256().update(b4a.from(value, 'utf8')).digest(digest);
  return b4a.toString(digest, 'hex');
};

const hmacSha256Hex = (key, value) => {
  const digest = b4a.alloc(Sha256.SHA256_BYTES);
  Sha256.HMAC(b4a.from(key, 'utf8'))
    .update(b4a.from(value, 'utf8'))
    .digest(digest);
  return b4a.toString(digest, 'hex');
};

const secureRandomBytes = (length) => {
  const bytes = b4a.alloc(length);
  sodium.randombytes_buf(bytes);
  return bytes;
};

export const createInternalStripeAuthHeaders = ({
  endpoint,
  body,
  secret,
  timestampSeconds = Math.floor(Date.now() / 1_000),
  nonceBytes = secureRandomBytes(32),
}) => {
  if (!b4a.isBuffer(nonceBytes) || nonceBytes.byteLength !== 32) {
    throw new Error('Stripe internal auth nonce must contain exactly 32 bytes.');
  }
  const timestamp = String(timestampSeconds);
  const nonce = b4a.toString(nonceBytes, 'hex');
  const message = [
    'mayhem-paygate-internal-request-v1',
    timestamp,
    nonce,
    'POST',
    endpoint.pathname,
    sha256Hex(body),
  ].join('\n');
  return {
    'x-mayhem-paygate-timestamp': timestamp,
    'x-mayhem-paygate-nonce': nonce,
    'x-mayhem-paygate-signature': hmacSha256Hex(secret, message),
  };
};
