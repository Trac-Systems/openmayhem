import b4a from 'b4a';
import { safeDecodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import nodeEntryUtils from 'trac-msb/src/core/state/utils/nodeEntry.js';
import { createHash } from 'trac-peer/src/utils/types.js';
import { adminContractTxDigest, CONTRACT_VERSION } from '../contract/contract.js';
import { getStateValue } from './rpc.js';

const hex = (value) => b4a.isBuffer(value) ? b4a.toString(value, 'hex') : String(value ?? '').toLowerCase();
const au = (value) => {
  if (!b4a.isBuffer(value) || value.length !== 16) throw new Error('Invalid MSB amount encoding.');
  return BigInt(`0x${hex(value)}`).toString();
};
const bounded = async (operation, label, milliseconds = 20000) => {
  let timer;
  try {
    return await Promise.race([operation, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${label} timed out; outcome remains uncertain.`)), milliseconds);
    })]);
  } finally { clearTimeout(timer); }
};

export class WriterCheckpointTransport {
  constructor({ peer, feature, releaseIdentity }) {
    this.peer = peer;
    this.feature = feature;
    this.releaseIdentity = releaseIdentity;
  }

  async state(key) {
    return (await bounded(getStateValue(this.peer, key, { confirmed: true }), 'Confirmed subnet read')).value;
  }

  async identity() {
    const admin = await this.state('admin');
    const publicKey = hex(this.peer.wallet.publicKey);
    if (admin !== publicKey || !this.peer.base.writable) {
      throw new Error('Paid checkpoint worker requires the canonical writable admin peer.');
    }
    if (this.releaseIdentity.contractVersion !== CONTRACT_VERSION) {
      throw new Error('Paid checkpoint release identity does not match the executing contract.');
    }
    return { public_key: publicKey, network_id: this.peer.msbClient.networkId,
      subnet_bootstrap: hex(this.peer.config.bootstrap), msb_bootstrap: this.peer.msbClient.bootstrapHex,
      address: this.peer.msbClient.pubKeyHexToAddress(publicKey) };
  }

  async history() {
    return { current: await this.state('checkpoint/current'), preparing: await this.state('checkpoint/preparing') };
  }

  async funding() {
    const address = this.peer.msbClient.pubKeyHexToAddress(hex(this.peer.wallet.publicKey));
    const length = this.peer.msbClient.getSignedLength();
    const raw = await bounded(this.peer.msbClient.getSignedAtLength(address, length), 'MSB balance read');
    const entry = raw?.value ? nodeEntryUtils.decode(raw.value) : null;
    return { address, balance_au: entry ? au(entry.balance) : '0',
      fee_au: au(this.peer.msbClient.getFee()), signed_length: length };
  }

  async prepareSnapshot(slot, observedAt) {
    const key = `checkpoint/prepared/${slot}`;
    const existing = await this.state(key);
    if (existing) return existing;
    const identity = await this.identity();
    const unsigned = {
      op: 'admin_contract_tx', prepared_command: {
        type: 'prepareStateCheckpoint', value: { op: 'prepare_state_checkpoint', slot,
          observed_at: observedAt, contract_code_sha256: this.releaseIdentity.contractCodeSha256 },
      }, address: identity.public_key, nonce: this.peer.protocol.instance.generateNonce(), sim: false,
      context: { contract_version: CONTRACT_VERSION, network_id: identity.network_id,
        subnet_bootstrap: identity.subnet_bootstrap, msb_bootstrap: identity.msb_bootstrap },
    };
    const tx = await adminContractTxDigest(unsigned);
    const signed = { ...unsigned, tx, signature: hex(this.peer.wallet.sign(b4a.from(tx, 'hex'))) };
    await bounded(this.feature.submit(`admin/contract-tx/${tx}`, signed), 'Checkpoint snapshot preparation');
    const snapshot = await this.state(key);
    if (!snapshot) throw new Error('Checkpoint snapshot is awaiting subnet confirmation.');
    return snapshot;
  }

  async preparePayment(snapshot) {
    const prepared = await bounded(this.peer.protocol.instance.preparePaidTransaction({
      type: 'stateCheckpoint', value: { op: 'state_checkpoint', slot: snapshot.slot,
        snapshot_hash: snapshot.snapshot_hash },
    }), 'MSB payment preparation');
    await this.validatePrepared(prepared);
    const simulated = await bounded(this.peer.protocol.instance.simulatePreparedTransaction(prepared),
      'Paid checkpoint simulation');
    if (simulated?.ok !== true) {
      throw new Error(`Paid checkpoint simulation rejected: ${simulated?.message ?? 'missing success'}`);
    }
    return prepared;
  }

  async validatePrepared(prepared) {
    const identity = await this.identity();
    const { surrogate: s, dispatch, payload } = prepared ?? {};
    if (prepared?.schema_version !== 1 || prepared.network_id !== identity.network_id ||
        s?.address !== identity.public_key || s?.bs !== identity.subnet_bootstrap ||
        s?.mbs !== identity.msb_bootstrap || dispatch?.type !== 'stateCheckpoint' ||
        dispatch?.value?.op !== 'state_checkpoint' || !Number.isSafeInteger(dispatch.value.slot) ||
        dispatch.value.slot < 1 || !/^[0-9a-f]{64}$/.test(dispatch.value.snapshot_hash ?? '')) {
      throw new Error('Persisted paid checkpoint has an invalid command or network identity.');
    }
    const contentHash = await createHash(this.peer.protocol.instance.safeJsonStringify(dispatch));
    const expectedTx = await this.peer.protocol.instance.generateTx(prepared.network_id,
      s.txv, s.iw, contentHash, s.bs, s.mbs, s.nonce);
    if (s.ch !== contentHash || s.tx !== expectedTx ||
        !this.peer.wallet.verify(s.signature, b4a.from(s.tx, 'hex'), s.address)) {
      throw new Error('Persisted paid checkpoint bytes or signature changed.');
    }
    const expectedPayload = { type: 12, address: identity.address,
      txo: { tx: s.tx, txv: s.txv, iw: s.iw, in: s.nonce, ch: s.ch,
        is: s.signature, bs: s.bs, mbs: s.mbs } };
    if (JSON.stringify(payload) !== JSON.stringify(expectedPayload)) {
      throw new Error('Persisted MSB payload differs from its signed preparation.');
    }
    if (dispatch.value.contract_version !== CONTRACT_VERSION) {
      const replay = { type: 'tx', key: s.tx, value: {
        dispatch, ipk: s.address,
      } };
      const storage = { get: async (key) => {
        const value = await this.state(key);
        return value === null ? null : { value };
      } };
      if (!await this.peer.contract.instance.isPreparedCheckpointReplay(replay, storage)) {
        throw new Error('Persisted paid checkpoint has no matching historical canonical preparation.');
      }
    }
    return identity;
  }

  async inspect(prepared, expectedFeeAu) {
    const identity = await this.validatePrepared(prepared);
    const length = this.peer.msbClient.getSignedLength();
    const tx = prepared.surrogate.tx;
    const record = await bounded(this.peer.msbClient.getSignedAtLength(tx, length), 'MSB transaction reconciliation');
    if (!record) {
      const changed = await bounded(this.peer.msbClient.getTxvHex(), 'MSB context read') !== prepared.surrogate.txv;
      const replacementEvidence = changed ? await bounded(
        this.peer.msbClient.getUnexecutedContextChange(tx, prepared.surrogate.txv),
        'Signed MSB context reconciliation'
      ) : null;
      return { confirmed: false, context_changed: changed, replacement_evidence: replacementEvidence };
    }
    if (!b4a.isBuffer(record.value)) throw new Error('MSB transaction proof is not encoded consensus evidence.');
    const decoded = safeDecodeApplyOperation(record.value);
    if (decoded?.type !== 12 || decoded.address?.toString('ascii') !== identity.address ||
        Object.entries(prepared.payload.txo).some(([key, value]) => hex(decoded.txo?.[key]) !== value)) {
      throw new Error('Confirmed MSB transaction differs from the exact paid checkpoint.');
    }
    const validator = this.peer.msbClient.addressToPubKeyHex(decoded.txo.va?.toString('ascii'));
    if (!/^[0-9a-f]{64}$/.test(validator ?? '')) throw new Error('MSB proof has no valid validator identity.');
    if (au(this.peer.msbClient.getFee()) !== expectedFeeAu) throw new Error('MSB fee evidence changed.');
    return { confirmed: true, proof: { tx, signed_length: length,
      operation_hex: hex(record.value), validator, fee_au: expectedFeeAu,
      network_id: identity.network_id, msb_bootstrap: identity.msb_bootstrap } };
  }

  async reconcileSubnet(prepared, proof) {
    await this.validatePrepared(prepared);
    const tx = prepared.surrogate.tx;
    const slot = prepared.dispatch.value.slot;
    const checkpoint = await this.state(`checkpoint/slot/${slot}`);
    if (checkpoint) {
      if (checkpoint.type !== 'paid_state_checkpoint' || checkpoint.tx !== tx ||
          checkpoint.snapshot_hash !== prepared.dispatch.value.snapshot_hash ||
          checkpoint.paid_by !== prepared.surrogate.address) {
        throw new Error('Canonical checkpoint conflicts with the paid MSB transaction.');
      }
      return checkpoint;
    }
    const index = await this.state(`tx/${tx}`);
    if (index !== null) {
      const result = await this.state(`txi/${index}`);
      if (result?.err !== null && result?.err !== undefined) {
        throw new Error(`Checkpoint fee paid but contract rejected: ${String(result.err)}`);
      }
    }
    // Recovery after a crash/lost ACK requires only the original MSB proof.
    // This subnet append is free and follows the peer's normal verified TX path.
    await bounded(this.peer.base.append({ type: 'tx', key: tx, value: {
      msbsl: proof.signed_length, dispatch: prepared.dispatch,
      ipk: prepared.surrogate.address, wp: proof.validator,
    } }), 'Checkpoint subnet proof application');
    return null;
  }

  async broadcast(prepared) {
    await this.validatePrepared(prepared);
    return await bounded(this.peer.protocol.instance.broadcastPreparedTransaction(prepared), 'MSB checkpoint broadcast');
  }
}
