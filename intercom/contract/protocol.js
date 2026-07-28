import { Protocol } from 'trac-peer';
import { CONTRACT_VERSION } from './contract.js';

const DEFAULT_MAYHEM_TX_MAX_BYTES = 64_000;
const DEFAULT_MAYHEM_FEATURE_MAX_BYTES = 64_000;

export const mayhemFeatureParticipant = (value) => {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  if (value.op === 'record_usage_receipt') {
    const provider = String(value.receipt?.body?.provider ?? '').toLowerCase();
    return /^[0-9a-f]{64}$/.test(provider) ? provider : null;
  }
  if (value.op === 'close_usage_reservation') {
    const provider = String(value.provider ?? '').toLowerCase();
    const actor = String(value.actor ?? '').toLowerCase();
    if (value.actor_role === 'provider' && actor === provider &&
        /^[0-9a-f]{64}$/.test(provider)) return provider;
    return null;
  }
  if (value.op === 'expire_usage_reservation') {
    const user = String(value.user ?? '').toLowerCase();
    return value.actor_role === 'user' &&
      String(value.actor ?? '').toLowerCase() === user &&
      /^[0-9a-f]{64}$/.test(user)
      ? user
      : null;
  }
  if ([
    'prepare_targeted_payout',
    'prepare_targeted_payout_epoch',
    'prepare_targeted_fiat_attempt',
    'finalize_targeted_fiat_attempt',
    'settle_targeted_tnk_output',
    'settle_targeted_fiat_output',
    'close_targeted_payout_epoch',
  ].includes(value.op)) {
    const admin = String(value.admin ?? '').toLowerCase();
    return /^[0-9a-f]{64}$/.test(admin) ? admin : null;
  }
  return null;
};

function positiveEnvInteger(name, fallback) {
  const raw = globalThis?.process?.env?.[name];
  if (raw === undefined || raw === '') return fallback;
  const value = Number.parseInt(raw, 10);
  return Number.isSafeInteger(value) && value > 0 ? value : fallback;
}

class MayhemProtocol extends Protocol {
  constructor(peer, base, options = {}) {
    super(peer, base, options);
  }

  txMaxBytes() {
    return positiveEnvInteger('MAYHEM_TX_MAX_BYTES', DEFAULT_MAYHEM_TX_MAX_BYTES);
  }

  versionedTransactionObject(value) {
    if (!value || typeof value !== 'object' || Array.isArray(value) ||
        !value.value || typeof value.value !== 'object' || Array.isArray(value.value)) {
      return value;
    }
    return {
      ...value,
      value: {
        ...value.value,
        contract_version: CONTRACT_VERSION,
      },
    };
  }

  async simulateTransaction(validatorPublicKey, value, surrogate = null) {
    return await super.simulateTransaction(
      validatorPublicKey,
      this.versionedTransactionObject(value),
      surrogate
    );
  }

  async broadcastTransaction(value, sim = false, surrogate = null) {
    return await super.broadcastTransaction(
      this.versionedTransactionObject(value),
      sim,
      surrogate
    );
  }

  featMaxBytes() {
    return positiveEnvInteger(
      'MAYHEM_FEATURE_MAX_BYTES',
      DEFAULT_MAYHEM_FEATURE_MAX_BYTES
    );
  }

  async extendApi() {
    this.api.getMayhemInfo = () => ({
      name: 'mayhem',
      contract: 'mayhem',
      phase: 'P1.1',
    });
  }

  mapTxCommand(command) {
    if (command === 'noop') {
      return {
        type: 'noop',
        value: { op: 'noop' },
      };
    }
    if (command === 'gated_noop') {
      return {
        type: 'gatedNoop',
        value: { op: 'gated_noop' },
      };
    }

    const json = this.safeJsonParse(command);
    if (json?.op === 'noop') {
      return {
        type: 'noop',
        value: { op: 'noop' },
      };
    }
    if (json?.op === 'gated_noop') {
      return {
        type: 'gatedNoop',
        value: { op: 'gated_noop' },
      };
    }
    if (json?.op === 'set_rules') {
      return {
        type: 'setRules',
        value: json,
      };
    }
    if (json?.op === 'set_params') {
      return {
        type: 'setParams',
        value: json,
      };
    }
    if (json?.op === 'set_payments') {
      return {
        type: 'setPayments',
        value: json,
      };
    }
    if (json?.op === 'read_params') {
      return {
        type: 'readParams',
        value: json,
      };
    }
    if (
      json?.op === 'consent' ||
      json?.op === 'register_provider' ||
      json?.op === 'tap_account_bind' ||
      json?.op === 'record_usage_receipt' ||
      json?.op === 'close_usage_reservation' ||
      json?.op === 'expire_usage_reservation'
    ) {
      return null;
    }
    if (json?.op === 'set_provider_rails') {
      return {
        type: 'setProviderRails',
        value: json,
      };
    }
    if (json?.op === 'set_provider_kyb') {
      return {
        type: 'setProviderKyb',
        value: json,
      };
    }
    if (json?.op === 'revoke_provider_kyb') {
      return {
        type: 'revokeProviderKyb',
        value: json,
      };
    }
    if (json?.op === 'ban_provider') {
      return {
        type: 'banProvider',
        value: json,
      };
    }
    if (json?.op === 'set_model_ref') {
      return {
        type: 'setModelRef',
        value: json,
      };
    }
    if (json?.op === 'publish_catalog') {
      return {
        type: 'publishCatalog',
        value: json,
      };
    }
    if (json?.op === 'register_enclave') {
      return {
        type: 'registerEnclave',
        value: json,
      };
    }
    if (
      json?.op === 'join_enclave' ||
      json?.op === 'leave_enclave' ||
      json?.op === 'join_room' ||
      json?.op === 'leave_room'
    ) {
      return null;
    }
    if (json?.op === 'update_enclave') {
      return {
        type: 'updateEnclave',
        value: json,
      };
    }
    if (json?.op === 'retire_enclave') {
      return {
        type: 'retireEnclave',
        value: json,
      };
    }
    if (json?.op === 'open_room') {
      return {
        type: 'openRoom',
        value: json,
      };
    }
    if (json?.op === 'close_room') {
      return {
        type: 'closeRoom',
        value: json,
      };
    }
    if (json?.op === 'set_price') {
      return {
        type: 'setPrice',
        value: json,
      };
    }
    if (json?.op === 'read_price') {
      return {
        type: 'readPrice',
        value: json,
      };
    }
    if (json?.op === 'record_rep_event') {
      return {
        type: 'recordReputationEvent',
        value: json,
      };
    }
    if (json?.op === 'anchor_reputation') return null;
    if (json?.op === 'auditor_register') {
      return {
        type: 'auditorRegister',
        value: json,
      };
    }
    if (json?.op === 'probe_result') {
      return {
        type: 'probeResult',
        value: json,
      };
    }
    if (json?.op === 'apply_targeted_epoch') return null;
    if (json?.op === 'epoch_commit') {
      return {
        type: 'epochCommit',
        value: json,
      };
    }
    if (json?.op === 'epoch_seal_empty') {
      return {
        type: 'epochSealEmpty',
        value: json,
      };
    }
    if (json?.op === 'fraud_proof') {
      return {
        type: 'fraudProof',
        value: json,
      };
    }
    if (json?.op === 'dispute') {
      return {
        type: 'dispute',
        value: json,
      };
    }
    if (json?.op === 'dispute_resolve') {
      return {
        type: 'disputeResolve',
        value: json,
      };
    }
    if (json?.op === 'dispute_expire') {
      return {
        type: 'disputeExpire',
        value: json,
      };
    }
    if (json?.op === 'rate_oracle' || json?.op === 'tap_rate_oracle') return null;
    if (
      json?.op === 'publish_payout_context' ||
      json?.op === 'schedule_payout_parameter' ||
      json?.op === 'verify_stripe_payout' ||
      json?.op === 'bind_provider_payout' ||
      json?.op === 'spend_reserve_targeted' ||
      json?.op === 'prepare_targeted_payout' ||
      json?.op === 'prepare_targeted_payout_epoch' ||
      json?.op === 'prepare_targeted_fiat_attempt' ||
      json?.op === 'finalize_targeted_fiat_attempt' ||
      json?.op === 'settle_targeted_tap' ||
      json?.op === 'settle_targeted_tnk_output' ||
      json?.op === 'settle_targeted_fiat_output' ||
      json?.op === 'close_targeted_payout_epoch'
    ) return null;
    if (
      json?.op === 'deposit_tnk' ||
      json?.op === 'tnk_deposit' ||
      json?.op === 'tap_deposit' ||
      json?.op === 'tap_deposit_reversal' ||
      json?.op === 'fiat_deposit'
    ) return null;
    if (json?.op === 'fiat_chargeback') {
      return {
        type: 'fiatChargeback',
        value: json,
      };
    }
    if (json?.op === 'read_key') {
      return {
        type: 'readKey',
        value: json,
      };
    }
    return null;
  }

  async printOptions() {
    console.log(' ');
    console.log('- Mayhem Commands:');
    console.log('- /tx --command "noop" --sim 1 | round-trips the Mayhem no-op contract command.');
    console.log('- /tx --command "gated_noop" --sim 1 | validates current-rules consent before no-op.');
    console.log('- /tx --command \'{ "op": "set_rules", "ver": 1, "hash": "<hash>" }\' --sim 1 | sets the active rules version.');
    console.log('- /tx --command \'{ "op": "set_params", "submitted_at": 0, "effective_at": 86400, "values": { "fee_bps": 1500 } }\' --sim 1 | schedules parameter changes.');
    console.log('- /tx --command \'{ "op": "set_payments", "ver": 1, "fiat": { "processor": "stripe", "integration_currency": "usd", "adaptive_pricing": true, "payout_currencies": ["eur","gbp","usd"], "locale": "en" }, "tap": { "chain_id": 1, "token_address": "<address>", "pool_address": "<address>" }, "tnk": { "network": "mainnet", "treasury_address": "<trac1-address>" } }\' --sim 1 | admin publishes canonical payment-rail discovery at payments/current; Stripe controls buyer-local Adaptive Pricing presentment while payout_currencies governs verified provider settlement.');
    console.log('- /tx --command \'{ "op": "read_params", "at": 86400, "keys": ["fee_bps"] }\' --sim 1 | reads active parameters at a timestamp.');
    console.log('- Consent, provider lifecycle, verified payout bindings, targeted spend reservations, targeted epoch apply, rate updates, revision-bound settlements, and reputation anchors are free Mayhem Feature records submitted by the Mayhem CLI/RPC feature path, not /tx commands.');
    console.log('- /tx --command \'{ "op": "set_provider_rails", "rails": ["fiat","tap","tnk"] }\' --sim 1 | provider declares the payment rails they accept for serving.');
    console.log('- Feature { "feature": "mayhem", "key": "payout/binding/<rail>/<provider>/<revision>", "value": { "op": "bind_provider_payout", "intent": { ... }, "provider_signature": "<sig>" } } | provider binds a verified immutable payout target; TAP/TNK target wallets co-sign and Stripe supplies processor verification.');
    console.log('- /tx --command \'{ "op": "set_provider_kyb", "provider": "<pubkey>", "legal_name": "Acme AI GmbH", "jurisdiction": "DE", "proof_hash": "<blake3>", "kyb_ref": "<off-ledger-ref>", "verified_at": 3600, "admin_sig": "<sig>" }\' --sim 1 | admin verifies provider business identity for Tier 4 accountability.');
    console.log('- /tx --command \'{ "op": "revoke_provider_kyb", "provider": "<pubkey>", "reason_hash": "<hash>" }\' --sim 1 | admin revokes provider KYB and drops them back to their hardware tier.');
    console.log('- /tx --command \'{ "op": "ban_provider", "provider": "<pubkey>", "reason_hash": "<hash>" }\' --sim 1 | bans a provider from future serving mutations.');
    console.log('- /tx --command \'{ "op": "set_model_ref", "model_id": "<catalog-model-id>", "rate_map": [{ "unit": "input_token", "per_unit_au": "18", "granularity": 1000 }, { "unit": "output_token", "per_unit_au": "55", "granularity": 1000 }] }\' --sim 1 | admin seeds catalog reference pricing in au_usd.');
    console.log('- /tx --command \'{ "op": "publish_catalog", "catalog_id": "mayhem-models", "source_kind": "huggingface", "catalog_url": "https://...", "signature_url": "https://...", "catalog_hash": "<blake3>", "signature_hash": "<blake3>", "key_id": "<id>", "public_key": "<hex>", "model_count": 3, "artifact_count": 5, "canaries": [{ "set_id": "canary-launch-v1", "url": "https://...", "hash": "<blake3>" }] }\' --sim 1 | admin anchors the latest signed catalog release for network discovery.');
    console.log('- /tx --command \'{ "op": "register_enclave", ... }\' --sim 1 | admin registers an enclave catalog entry.');
    console.log('- /tx --command \'{ "op": "open_room", "enclave_id": "<id>", "nonce": "<nonce>", "label": "<label>", "policy": {} }\' --sim 1 | admin opens a canonical room for one admin enclave.');
    console.log('- /tx --command \'{ "op": "close_room", "room_id": "<room_id>" }\' --sim 1 | admin closes a canonical room.');
    console.log('- /tx --command \'{ "op": "set_price", "enclave_id": "<id>", "rate_map": [{ "unit": "input_token", "per_unit_au": "18", "granularity": 1000 }, { "unit": "output_token", "per_unit_au": "55", "granularity": 1000 }], "per_req_au": "0", "min_session_au": "100", "effective_at": 21600 }\' --sim 1 | admin sets an enclave price in au_usd.');
    console.log('- /tx --command \'{ "op": "read_price", "enclave_id": "<id>", "at": 21600 }\' --sim 1 | reads the active enclave price at a timestamp.');
    console.log('- /tx --command \'{ "op": "record_rep_event", "provider": "<pubkey>", "event_id": "<id>", "kind": "session_ok", "paid_au": "1000", "epoch": 1, "at": 3600 }\' --sim 1 | admin/oracle records reputation evidence.');
    console.log('- /tx --command \'{ "op": "auditor_register", "auditor": "<pubkey>" }\' --sim 1 | admin accredits an auditor, or a qualified peer self-registers.');
    console.log('- /tx --command \'{ "op": "probe_result", "probe_id": "<id>", "probe_kind": "canary", "provider": "<pk>", "enclave_id": "<id>", "binary_hash": "<hash>", "verification_method": "token_fingerprint", "match_bps": 9700, "pass": true, "canary_set": "canary-dev-v1", "session_receipt_hash": "<receipt-hash>", "evidence_hash": "<hash>", "auditor_sig": "<sig>", "epoch": 1, "at": 3600 }\' --sim 1 | auditor submits signed paid-session canary evidence.');
    console.log('- /tx --command \'{ "op": "epoch_commit", "epoch": 1, "at": 3600, "roots": { "dep": "<root>", "use": "<root>", "earn": "<root>", "fee": "<root>" }, "totals": { "dep_count": 0, "dep_au": "0", "use_count": 1, "use_au": "1000", "provider_count": 1, "earn_au": "850", "fee_au": "150", "fee_cum_au": "150" } }\' --sim 1 | permissionlessly anchors epoch roots.');
    console.log('- /tx --command \'{ "op": "epoch_seal_empty", "epoch": 1, "at": 3600, "reason_hash": "<blake3>" }\' --sim 1 | admin seals one elapsed empty/unsubmittable epoch so later epochs can settle.');
    console.log('- /tx --command \'{ "op": "fraud_proof", "epoch": 1, "proof_epoch": 2, "at": 7200, "reason": "over_credit", "receipt": { ... }, "claimed_au_owed_cum": "2000" }\' --sim 1 | submits a signed receipt proving an inflated epoch commit.');
    console.log('- /tx --command \'{ "op": "dispute", "session_id": "<id>", "reason": "service_failure", "provider": "<pk>", "at": 7200, "evidence_hash": "<hash>" }\' --sim 1 | opens a dispute with the admin-governed refundable AU bond.');
    console.log('- /tx --command \'{ "op": "dispute_resolve", "dispute_id": 1, "outcome": "provider_fault", "deposit_action": "refund", "rationale_hash": "<hash>", "slash": true, "at": 10800 }\' --sim 1 | admin resolves a dispute.');
    console.log('- /tx --command \'{ "op": "dispute_expire", "dispute_id": 1, "at": 10800 }\' --sim 1 | permissionlessly refunds an unresolved dispute bond after its timeout epoch.');
    console.log('- Feature { "feature": "mayhem", "key": "epoch/targeted/apply/<epoch>/<payload_hash>", "value": { "op": "apply_targeted_epoch", "epoch": 1, "at": 3600, "debits": [{ "rail": "fiat", "user": "<pk>", "au": "1000" }], "earnings": [{ "rail": "fiat", "provider": "<pk>", "payout_revision": "<revision>", "gross_au": "1000" }], "allocations": [{ ... }] } } | admin/oracle applies exact session allocations bound to immutable payout revisions.');
    console.log('- Feature { "feature": "mayhem", "key": "hold/targeted/<rail>/<user>/<epoch>/<session_id>/<payload_hash>", "value": { "op": "spend_reserve_targeted", "contract_version": 14, "payout_revision": "<revision>", "session_id": "<id>", "epoch": 1, "rail": "fiat", "user": "<pk>", "provider": "<pk>", "enclave_id": "<id>", "price_ver": 1, "rules_ver": 1, "max_spend_au": "1000", "voucher": { ... }, "provider_sig": "<sig>" } } | provider reserves user balance against the exact active payout revision before serving.');
    console.log('- Feature { "feature": "mayhem", "key": "rate/tnk/<ts>/<payload_hash>", "value": { "op": "rate_oracle", "tnk_usd_au": "50000000000000000", "source": "gate-spot", "ts": 3600 } } | admin/oracle updates the TNK/USD rate for free.');
    console.log('- Feature { "feature": "mayhem", "key": "rate/tap/<ts>/<payload_hash>", "value": { "op": "tap_rate_oracle", "tap_usd_au": "50000000000000000", "source": "uniswap-v2-twap-median", "ts": 3600 } } | admin/oracle updates the TAP/USD policy rate for free.');
    console.log('- Feature { "feature": "mayhem", "key": "rep/<provider>/<epoch>/<payload_hash>", "value": { "op": "anchor_reputation", "provider": "<pubkey>", "epoch": 1, "folded_at": 3600, "events_head": "<head>", "r_bps": 8700, "raw_milli": 12345, "successful_sessions": 50 } } | admin/oracle anchors a folded rep/<provider> snapshot for free.');
    console.log('- Feature { "feature": "mayhem", "key": "dep/tnk-intent/<memo>/<payload_hash>", "value": { "op": "deposit_tnk", "sender": "<pk>", "intent": { ... }, "sig": "<sig>" } } | user-signed TNK deposit intent for free.');
    console.log('- Feature { "feature": "mayhem", "key": "dep/tnk|tap|fiat/...", "value": { "op": "tnk_deposit|tap_deposit|fiat_deposit", ... } } | admin/oracle credits deposit evidence for free; deposits fold into ev/dep roots.');
    console.log('- prepare_targeted_payout_epoch freezes one signed-length-pinned TNK/fiat plan; settle_targeted_tnk_output and settle_targeted_fiat_output record one irreversible output at a time; close_targeted_payout_epoch closes only a complete or explicit carry/no-work plan.');
    console.log('- prepare_targeted_fiat_attempt and finalize_targeted_fiat_attempt bind numbered Stripe attempts; only succeeded attempts settle, and renewal requires definitive expired_pre_effect state. TAP remains one atomic settle_targeted_tap root.');
    console.log('- /tx --command \'{ "op": "read_key", "key": "<key>" }\' --sim 1 | reads a contract key.');
    console.log('- /sc_join --channel "<name>" | join an ephemeral sidechannel.');
    console.log('- /sc_open --channel "<name>" [--via "<channel>"] | request others to open a sidechannel.');
    console.log('- /sc_send --channel "<name>" --message "<text>" | send a sidechannel message.');
    console.log('- /sc_stats | show sidechannel channels + connection count.');
  }

  async customCommand(input) {
    if (this.input !== null) {
      return null;
    }
    this.input = input;

    if (this.input.startsWith('/sc_stats')) {
      const channels = this.peer?.sidechannel ? Array.from(this.peer.sidechannel.channels.keys()) : [];
      const connectionCount = this.peer?.sidechannel?.connections?.size ?? 0;
      console.log({ channels, connectionCount });
      this.input = null;
      return { channels, connectionCount };
    }

    this.input = null;
    return null;
  }
}

export default MayhemProtocol;
