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
    if (json?.op === 'register_enclave') {
      return {
        type: 'registerEnclave',
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
    console.log('- /tx --command \'{ "op": "register_provider", "payout_addr": "<target>", "payout_method": "tnk" }\' --sim 1 | registers a provider.');
    console.log('- /tx --command \'{ "op": "register_enclave", ... }\' --sim 1 | registers an enclave.');
    console.log('- /tx --command \'{ "op": "open_room", "model_id": "<model>", "nonce": "<nonce>", "label": "<label>", "policy": {} }\' --sim 1 | opens a model room.');
    console.log('- /tx --command \'{ "op": "close_room", "room_id": "<room_id>" }\' --sim 1 | closes a model room.');
    console.log('- /tx --command \'{ "op": "set_price", "enclave_id": "<id>", "in_per_1k_mu": 18, "out_per_1k_mu": 55, "per_req_mu": 0, "min_session_mu": 100, "effective_at": 21600 }\' --sim 1 | sets an enclave price.');
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
