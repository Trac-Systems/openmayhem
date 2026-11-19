import assert from 'node:assert/strict';
import { blake3 } from '@noble/hashes/blake3';
import { stableJson } from '../../../intercom/scripts/recompute-epoch-roots.mjs';

// Synthetic Core responses for the in-process EVM. The immutable oracle key
// uses the same canonical bytes a signed native rate feature commits.
export function oracleRecord(lock) {
  const value = { op: 'tap_rate_oracle', tap_usd_au: lock.tap_usd_au, source: lock.source, ts: lock.rate_ts };
  const digest = Buffer.from(blake3(Buffer.from(stableJson({ domain: 'mayhem-rate-feature-v1', value })))).toString('hex');
  return { denom: 'tap_usd_au', tap_usd_au: lock.tap_usd_au, source: lock.source, ts: lock.rate_ts,
    updated_at: `rate/tap/${lock.rate_ts}/${digest}`, posted_by: lock.posted_by, posted_by_role: 'admin' };
}

export function pricingFixture(lock) {
  const rate = oracleRecord(lock);
  lock.rate_record_key = rate.updated_at;
  const state = new Map([
    ['admin', lock.posted_by], [rate.updated_at, rate],
    ['payments/current', { ver: lock.payment_config_ver, denom: 'au_usd', set_by: lock.posted_by,
      set_by_role: 'admin', tap: { chain_id: lock.chain_id, token_address: lock.token_address, pool_address: lock.pool_address } }],
  ]);
  const peerRpcUrl = 'http://fixture.invalid/v1';
  const fetchImpl = async url => {
    assert.equal(url.origin, new URL(peerRpcUrl).origin);
    const key = url.searchParams.get('key');
    assert.equal(url.searchParams.get('confirmed'), 'true');
    if (url.searchParams.has('signed_length')) assert.equal(url.searchParams.get('signed_length'), '44');
    else assert.equal(key, 'admin');
    return Response.json({ key, value: state.get(key) ?? null, confirmed: true, signed_length: 44 });
  };
  return { state, peerRpcUrl, fetchImpl };
}
