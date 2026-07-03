import { Protocol } from 'trac-peer';

class MayhemProtocol extends Protocol {
  constructor(peer, base, options = {}) {
    super(peer, base, options);
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
    if (json?.op === 'read_params') {
      return {
        type: 'readParams',
        value: json,
      };
    }
    if (json?.op === 'consent') {
      return {
        type: 'consent',
        value: json,
      };
    }
    if (json?.op === 'register_provider') {
      return {
        type: 'registerProvider',
        value: json,
      };
    }
    if (json?.op === 'set_provider_payout') {
      return {
        type: 'setProviderPayout',
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
    if (json?.op === 'register_enclave') {
      return {
        type: 'registerEnclave',
        value: json,
      };
    }
    if (json?.op === 'join_enclave') {
      return {
        type: 'joinEnclave',
        value: json,
      };
    }
    if (json?.op === 'leave_enclave') {
      return {
        type: 'leaveEnclave',
        value: json,
      };
    }
    if (json?.op === 'join_room') {
      return {
        type: 'joinRoom',
        value: json,
      };
    }
    if (json?.op === 'leave_room') {
      return {
        type: 'leaveRoom',
        value: json,
      };
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
    if (json?.op === 'anchor_reputation') {
      return {
        type: 'anchorReputation',
        value: json,
      };
    }
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
    if (json?.op === 'epoch_apply') {
      return {
        type: 'epochApply',
        value: json,
      };
    }
    if (json?.op === 'epoch_commit') {
      return {
        type: 'epochCommit',
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
    if (json?.op === 'rate_oracle') {
      return {
        type: 'rateOracle',
        value: json,
      };
    }
    if (json?.op === 'deposit_tnk') {
      return {
        type: 'depositTnk',
        value: json,
      };
    }
    if (json?.op === 'tnk_deposit') {
      return {
        type: 'tnkDeposit',
        value: json,
      };
    }
    if (json?.op === 'fiat_deposit') {
      return {
        type: 'fiatDeposit',
        value: json,
      };
    }
    if (json?.op === 'fiat_chargeback') {
      return {
        type: 'fiatChargeback',
        value: json,
      };
    }
    if (json?.op === 'payout_confirm') {
      return {
        type: 'payoutConfirm',
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
    console.log('- /tx --command \'{ "op": "read_params", "at": 86400, "keys": ["fee_bps"] }\' --sim 1 | reads active parameters at a timestamp.');
    console.log('- /tx --command \'{ "op": "consent", "ver": 1, "hash": "<hash>", "sig": "<sig>" }\' --sim 1 | records consent.');
    console.log('- /tx --command \'{ "op": "register_provider" }\' --sim 1 | provider opts into serving admin-created enclaves.');
    console.log('- /tx --command \'{ "op": "set_provider_payout", "provider": "<pubkey>", "payout_addr": "<target>", "payout_method": "tnk|stripe|coinbase" }\' --sim 1 | admin sets a provider payout target.');
    console.log('- /tx --command \'{ "op": "ban_provider", "provider": "<pubkey>", "reason_hash": "<hash>" }\' --sim 1 | bans a provider from future serving mutations.');
    console.log('- /tx --command \'{ "op": "set_model_ref", "model_id": "<catalog-model-id>", "price_ref_mu": { "in_per_1k": 18, "out_per_1k": 55 } }\' --sim 1 | admin seeds catalog reference pricing in mu_usd.');
    console.log('- /tx --command \'{ "op": "register_enclave", ... }\' --sim 1 | admin registers an enclave catalog entry.');
    console.log('- /tx --command \'{ "op": "join_enclave", "enclave_id": "<id>" }\' --sim 1 | provider opts into serving an existing enclave.');
    console.log('- /tx --command \'{ "op": "leave_enclave", "enclave_id": "<id>" }\' --sim 1 | provider stops serving an enclave.');
    console.log('- /tx --command \'{ "op": "join_room", "room_id": "<room_id>", "enclave_id": "<id>" }\' --sim 1 | provider joins a canonical admin room with a served enclave.');
    console.log('- /tx --command \'{ "op": "leave_room", "room_id": "<room_id>", "enclave_id": "<id>" }\' --sim 1 | provider leaves a canonical room.');
    console.log('- /tx --command \'{ "op": "open_room", "enclave_id": "<id>", "nonce": "<nonce>", "label": "<label>", "policy": {} }\' --sim 1 | admin opens a canonical room for one admin enclave.');
    console.log('- /tx --command \'{ "op": "close_room", "room_id": "<room_id>" }\' --sim 1 | admin closes a canonical room.');
    console.log('- /tx --command \'{ "op": "set_price", "enclave_id": "<id>", "in_per_1k_mu": 18, "out_per_1k_mu": 55, "per_req_mu": 0, "min_session_mu": 100, "effective_at": 21600 }\' --sim 1 | admin sets an enclave price in mu_usd.');
    console.log('- /tx --command \'{ "op": "read_price", "enclave_id": "<id>", "at": 21600 }\' --sim 1 | reads the active enclave price at a timestamp.');
    console.log('- /tx --command \'{ "op": "record_rep_event", "provider": "<pubkey>", "event_id": "<id>", "kind": "session_ok", "paid_mu": 1000, "epoch": 1, "at": 3600 }\' --sim 1 | admin/oracle records reputation evidence.');
    console.log('- /tx --command \'{ "op": "anchor_reputation", "provider": "<pubkey>", "epoch": 1, "folded_at": 3600, "events_head": "<head>", "r_bps": 8700, "raw_milli": 12345, "successful_sessions": 50 }\' --sim 1 | admin/oracle anchors a rep/<provider> snapshot.');
    console.log('- /tx --command \'{ "op": "auditor_register", "auditor": "<pubkey>" }\' --sim 1 | admin accredits an auditor, or a qualified peer self-registers.');
    console.log('- /tx --command \'{ "op": "probe_result", "probe_id": "<id>", "probe_kind": "canary", "provider": "<pk>", "enclave_id": "<id>", "match_bps": 9700, "pass": true, "canary_set": "canary-dev-v1", "session_receipt_hash": "<receipt-hash>", "evidence_hash": "<hash>", "epoch": 1, "at": 3600 }\' --sim 1 | auditor submits paid-session canary evidence.');
    console.log('- /tx --command \'{ "op": "epoch_commit", "epoch": 1, "at": 3600, "roots": { "dep": "<root>", "use": "<root>", "earn": "<root>", "fee": "<root>", "pay": "<root>" }, "totals": { "dep_count": 0, "dep_mu": 0, "use_count": 1, "use_mu": 1000, "provider_count": 1, "earn_mu": 850, "fee_mu": 150, "fee_cum_mu": 150, "pay_count": 0, "pay_mu": 0 } }\' --sim 1 | permissionlessly anchors epoch roots.');
    console.log('- /tx --command \'{ "op": "fraud_proof", "epoch": 1, "proof_epoch": 2, "at": 7200, "reason": "over_credit", "receipt": { ... }, "claimed_mu_owed_cum": 2000 }\' --sim 1 | submits a signed receipt proving an inflated epoch commit.');
    console.log('- /tx --command \'{ "op": "dispute", "session_id": "<id>", "reason": "service_failure", "provider": "<pk>", "at": 7200, "evidence_hash": "<hash>" }\' --sim 1 | opens a dispute with a refundable 5000 mu deposit.');
    console.log('- /tx --command \'{ "op": "dispute_resolve", "dispute_id": 1, "outcome": "provider_fault", "deposit_action": "refund", "rationale_hash": "<hash>", "slash": true, "at": 10800 }\' --sim 1 | admin resolves a dispute.');
    console.log('- /tx --command \'{ "op": "epoch_apply", "epoch": 1, "at": 3600, "debits": [{ "user": "<pk>", "mu": 1000 }], "earnings": [{ "provider": "<pk>", "gross_mu": 1000 }] }\' --sim 1 | admin/oracle applies bounded credit, earning, and fee deltas.');
    console.log('- /tx --command \'{ "op": "rate_oracle", "tnk_usd_e6": 50000, "source": "coinbase-spot", "ts": 3600 }\' --sim 1 | admin/oracle updates the TNK/USD rate.');
    console.log('- /tx --command \'{ "op": "deposit_tnk", "memo_hash": "<hash>" }\' --sim 1 | user creates a memo-bound TNK deposit intent.');
    console.log('- /tx --command \'{ "op": "tnk_deposit", "memo_hash": "<hash>", "tnk_e18": "1000000000000000000", "msb_tx_hash": "<hash>", "epoch": 1, "at": 3600 }\' --sim 1 | admin/oracle credits a memo-bound TNK deposit using a fresh rate.');
    console.log('- /tx --command \'{ "op": "payout_confirm", "epoch": 7, "who": "<pk>", "mu": 1000000, "tnk_e18": "500000000000000000", "msb_tx_hash": "<hash>", "at": 25200 }\' --sim 1 | admin/oracle confirms an automated TNK provider payout.');
    console.log('- /tx --command \'{ "op": "payout_confirm", "rail": "stripe", "epoch": 7, "who": "<pk>", "mu": 1000000, "external_ref": "tr_...", "at": 25200 }\' --sim 1 | admin/oracle confirms an automated Stripe Connect provider payout.');
    console.log('- /tx --command \'{ "op": "payout_confirm", "rail": "coinbase", "epoch": 7, "who": "<pk>", "mu": 1000000, "external_ref": "transfer_...", "at": 25200 }\' --sim 1 | admin/oracle confirms an automated Coinbase provider payout.');
    console.log('- /tx --command \'{ "op": "payout_confirm", "kind": "fee_sweep", "epoch": 7, "who": "treasury", "mu": 1000000, "tnk_e18": "500000000000000000", "msb_tx_hash": "<hash>", "at": 25200 }\' --sim 1 | admin/oracle confirms a router fee sweep.');
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
