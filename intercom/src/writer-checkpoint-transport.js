import b4a from 'b4a';
import { blake3 } from '@tracsystems/blake3';
import { safeDecodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import nodeEntryUtils from 'trac-msb/src/core/state/utils/nodeEntry.js';
import { isAcceptedTxContentHash } from 'trac-peer/src/utils/types.js';
import { adminContractTxDigest, CONTRACT_VERSION } from '../contract/contract.js';
import { getStateValue } from './rpc.js';

const hex = (value) => b4a.isBuffer(value) ? b4a.toString(value, 'hex') : String(value ?? '').toLowerCase();
const previousVersion = (prepared) => prepared?.dispatch?.value?.contract_version === CONTRACT_VERSION - 1;
const committedRecoveryVersion = prepared => CONTRACT_VERSION === 32 && prepared?.dispatch?.value?.contract_version === 30;
const signedLength = (core) => {
  if (!Number.isSafeInteger(core?.signedLength) || core.signedLength < 2) {
    throw new Error('Confirmed checkpoint recovery view is unavailable.');
  }
  return core.signedLength;
};
// A sparse replica raises BLOCK_NOT_AVAILABLE for a missing B-tree block.
// Only a completed keyed lookup returning null is evidence of absence.
async function localEntry(view, length, key) {
  if (!Number.isSafeInteger(length) || length < 2 || length > signedLength(view.core)) {
    throw new Error('Invalid signed checkpoint recovery checkout.');
  }
  const session = view.checkout(length);
  try {
    return await session.get(key, { wait: false, update: false });
  } finally { await session.close(); }
}
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
  constructor({ peer, msb, feature, releaseIdentity }) {
    this.peer = peer;
    this.msb = msb;
    this.feature = feature;
    this.releaseIdentity = releaseIdentity;
  }

  async state(key) {
    return (await bounded(getStateValue(this.peer, key, { confirmed: true }), 'Confirmed subnet read')).value;
  }

  async identity({ local = false } = {}) {
    const view = this.peer.base.view;
    const admin = local ? (await localEntry(view, signedLength(view.core), 'admin'))?.value : await this.state('admin');
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

  async funding({ local = false } = {}) {
    const address = this.peer.msbClient.pubKeyHexToAddress(hex(this.peer.wallet.publicKey));
    const length = this.peer.msbClient.getSignedLength();
    const raw = local ? await localEntry(this.msb.state.base.view, length, address) :
      await bounded(this.peer.msbClient.getSignedAtLength(address, length), 'MSB balance read');
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

  async validatePrepared(prepared, { inspectPrevious = false, validator, subnetLength } = {}) {
    const committed = committedRecoveryVersion(prepared);
    const identity = await this.identity({ local: inspectPrevious || committed });
    const { surrogate: s, dispatch, payload } = prepared ?? {};
    if (prepared?.schema_version !== 1 || prepared.network_id !== identity.network_id ||
        s?.address !== identity.public_key || s?.bs !== identity.subnet_bootstrap ||
        s?.mbs !== identity.msb_bootstrap || dispatch?.type !== 'stateCheckpoint' ||
        dispatch?.value?.op !== 'state_checkpoint' || !Number.isSafeInteger(dispatch.value.slot) ||
        dispatch.value.slot < 1 || !/^[0-9a-f]{64}$/.test(dispatch.value.snapshot_hash ?? '')) {
      throw new Error('Persisted paid checkpoint has an invalid command or network identity.');
    }
    const contentHashAccepted = await isAcceptedTxContentHash(
      this.peer.protocol.instance.safeJsonStringify(dispatch), s.ch, this.peer.config.txContentHash);
    const expectedTx = await this.peer.protocol.instance.generateTx(prepared.network_id,
      s.txv, s.iw, s.ch, s.bs, s.mbs, s.nonce);
    if (!contentHashAccepted || s.tx !== expectedTx ||
        !this.peer.wallet.verify(s.signature, b4a.from(s.tx, 'hex'), s.address)) {
      throw new Error('Persisted paid checkpoint bytes or signature changed.');
    }
    const expectedPayload = { type: 12, address: identity.address,
      txo: { tx: s.tx, txv: s.txv, iw: s.iw, in: s.nonce, ch: s.ch,
        is: s.signature, bs: s.bs, mbs: s.mbs } };
    if (JSON.stringify(payload) !== JSON.stringify(expectedPayload)) {
      throw new Error('Persisted MSB payload differs from its signed preparation.');
    }
    if (previousVersion(prepared)) {
      if (!inspectPrevious) throw new Error('Previous-contract checkpoint permits read-only reconciliation only.');
      if (![s.tx, s.txv, s.iw, s.nonce, s.ch, s.bs, s.mbs, s.address].every(
        value => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value)) ||
        typeof s.signature !== 'string' || !/^[0-9a-f]{128}$/.test(s.signature)) {
        throw new Error('Previous checkpoint contains a malformed signed field.');
      }
    } else if (dispatch.value.contract_version !== CONTRACT_VERSION) {
      const replay = { type: 'tx', key: s.tx, value: {
        dispatch, ipk: s.address,
      } };
      if (committed && validator !== undefined) replay.value.wp = validator;
      const view = this.peer.base.view;
      const length = committed ? subnetLength ?? signedLength(view.core) : null;
      const storage = { get: async (key) => {
        if (committed) return localEntry(view, length, key);
        const value = await this.state(key);
        return value === null ? null : { value };
      } };
      if (!await this.peer.contract.instance.isPreparedCheckpointReplay(replay, storage)) {
        throw new Error('Persisted paid checkpoint has no matching historical canonical preparation.');
      }
    }
    return identity;
  }

  async previousCheckpointState(prepared, proof = null) {
    const { dispatch, surrogate } = prepared;
    const { slot, snapshot_hash: hash } = dispatch.value;
    const view = this.peer.base.view;
    const length = signedLength(view.core);
    const read = async key => (await localEntry(view, length, key))?.value ?? null;
    const snapshot = await read(`checkpoint/prepared/${slot}`);
    const contract = this.peer.contract.instance;
    if (Object.keys(dispatch).length !== 2 || Object.keys(dispatch.value).length !== 4 ||
        await read('admin') !== surrogate.address || snapshot?.type !== 'state_checkpoint_snapshot' ||
        snapshot.schema_version !== 1 || snapshot.slot !== slot || snapshot.prepared_by !== surrogate.address ||
        snapshot.state?.contract_version !== dispatch.value.contract_version || snapshot.snapshot_hash !== hash) {
      throw new Error('Previous-contract checkpoint lacks its exact signed preparation.');
    }
    const { snapshot_hash, ...body } = snapshot;
    if (await contract.opaqueHash('mayhem-checkpoint-state-v1', snapshot.state) !== snapshot.state_hash ||
        await contract.opaqueHash('mayhem-checkpoint-snapshot-v1', body) !== snapshot_hash) {
      throw new Error('Previous-contract checkpoint snapshot digest mismatch.');
    }
    const checkpoint = await read(`checkpoint/slot/${slot}`);
    const index = await read(`tx/${surrogate.tx}`);
    if (checkpoint === null) {
      const preparing = await read('checkpoint/preparing');
      const previous = await read('checkpoint/current');
      if (preparing?.slot !== slot || preparing.snapshot_hash !== hash ||
          snapshot.previous_tx !== (previous?.tx ?? null) || (previous && previous.slot + 1 !== slot)) {
        throw new Error('Previous-contract checkpoint no longer matches the pending canonical slot.');
      }
      return { snapshot, checkpoint: null, indexed: index !== null };
    }
    if (!proof) return { snapshot, checkpoint, indexed: index !== null };
    if (checkpoint.type !== 'paid_state_checkpoint' || checkpoint.schema_version !== 1 ||
        checkpoint.slot !== slot || checkpoint.tx !== surrogate.tx || checkpoint.paid_by !== surrogate.address ||
        ['scheduled_at', 'observed_at', 'state_hash', 'snapshot_hash', 'previous_tx', 'no_change']
          .some(key => checkpoint[key] !== snapshot[key]) || !Number.isSafeInteger(index) || index < 0) {
      throw new Error('Previous-contract paid slot lacks matching signed application evidence.');
    }
    const result = await read(`txi/${index}`);
    if (result?.err !== null || result.tx !== surrogate.tx || result.ipk !== surrogate.address ||
        result.wp !== proof.validator || JSON.stringify(result.val) !== JSON.stringify(dispatch)) {
      throw new Error('Previous-contract paid checkpoint lacks exact indexed success.');
    }
    return { snapshot, checkpoint, indexed: true };
  }

  async previousContextEvidence(prepared, base) {
    const old = prepared.surrogate;
    const live = await this.peer.msbClient.getTxvHex();
    if (live === old.txv) return null;
    const system = base.system;
    const systemLength = signedLength(system.core);
    const systemKey = hex(system.core.key);
    const view = base.view;
    const viewKey = hex(view.core.key);
    if (![systemKey, viewKey].every(key => /^[0-9a-f]{64}$/.test(key))) return null;
    // SystemView.checkout opens the signed info record with wait:false.
    const state = await system.checkout(systemLength);
    let txv, linked;
    try {
      if (!state.indexers?.length || state.indexers.some(entry =>
        !b4a.isBuffer(entry.key) || entry.key.length !== 32)) return null;
      txv = hex(await blake3(b4a.concat(state.indexers.map(entry => entry.key))));
      linked = state.views.find(entry => hex(entry.key) === viewKey)?.length;
    } finally { await state.close(); }
    if (txv !== live || txv === old.txv || !Number.isSafeInteger(linked) || linked < 2) return null;
    if (await localEntry(view, linked, old.tx) !== null) return null;
    const checked = signedLength(view.core);
    if (await localEntry(view, checked, old.tx) !== null) return null;
    if (base.system !== system || base.view !== view || hex(system.core.key) !== systemKey ||
        hex(view.core.key) !== viewKey || await this.peer.msbClient.getTxvHex() !== txv) return null;
    return { type: 'unexecuted_at_signed_context_change', tx: old.tx, previous_txv: old.txv, txv,
      system_key: systemKey, system_signed_length: systemLength, view_key: viewKey,
      linked_view_signed_length: linked, checked_view_signed_length: checked };
  }

  async inspect(prepared, expectedFeeAu, active = null) {
    const previous = previousVersion(prepared);
    const committed = committedRecoveryVersion(prepared);
    const historical = previous || committed;
    const identity = await this.validatePrepared(prepared, { inspectPrevious: previous });
    if (historical && (active?.slot !== prepared.dispatch.value.slot ||
        active.snapshot_hash !== prepared.dispatch.value.snapshot_hash)) {
      throw new Error('Journal slot or snapshot differs from the previous-contract payment.');
    }
    const base = historical ? this.msb.state.base : null;
    const length = historical ? signedLength(base.view.core) : this.peer.msbClient.getSignedLength();
    const tx = prepared.surrogate.tx;
    const record = historical ? await localEntry(base.view, length, tx) :
      await bounded(this.peer.msbClient.getSignedAtLength(tx, length), 'MSB transaction reconciliation');
    if (!record) {
      if (previous) {
        const canonical = await this.previousCheckpointState(prepared);
        if (canonical.checkpoint || canonical.indexed || active.phase === 'MSB_CONFIRMED' || active.proof) {
          throw new Error('Previous checkpoint has application or payment evidence but the signed MSB record is unavailable.');
        }
        const evidence = await this.previousContextEvidence(prepared, base);
        return { confirmed: false, previous_version: true, context_changed: evidence !== null,
          replacement_evidence: evidence, canonical_snapshot: canonical.snapshot };
      }
      if (committed) {
        const view = this.peer.base.view;
        const subnetLength = signedLength(view.core);
        if (active.phase === 'MSB_CONFIRMED' || active.proof || await localEntry(view, subnetLength, `tx/${tx}`) ||
            await localEntry(view, subnetLength, `checkpoint/slot/${active.slot}`)) {
          throw new Error('Committed checkpoint has payment or application evidence but the signed MSB record is unavailable.');
        }
        const changed = await this.peer.msbClient.getTxvHex() !== prepared.surrogate.txv;
        const evidence = changed ? await this.previousContextEvidence(prepared, base) : null;
        const snapshot = evidence ? (await localEntry(view, subnetLength, `checkpoint/prepared/${active.slot}`))?.value : null;
        return { confirmed: false, committed_recovery: true, context_changed: changed,
          replacement_evidence: evidence, canonical_snapshot: snapshot };
      }
      const changed = await bounded(this.peer.msbClient.getTxvHex(), 'MSB context read') !== prepared.surrogate.txv;
      const replacementEvidence = changed ? await bounded(
        this.peer.msbClient.getUnexecutedContextChange(tx, prepared.surrogate.txv),
        'Signed MSB context reconciliation'
      ) : null;
      return { confirmed: false, context_changed: changed, replacement_evidence: replacementEvidence };
    }
    return { confirmed: true, previous_version: previous,
      proof: this.paymentProof(prepared, identity, length, record, expectedFeeAu) };
  }

  paymentProof(prepared, identity, length, record, expectedFeeAu) {
    if (!b4a.isBuffer(record?.value) || record.value.byteLength > this.peer.config.maxMsbApplyOperationBytes) {
      throw new Error('MSB transaction proof is not bounded encoded consensus evidence.');
    }
    const decoded = safeDecodeApplyOperation(record.value);
    if (decoded?.type !== 12 || decoded.address?.toString('ascii') !== identity.address ||
        Object.entries(prepared.payload.txo).some(([key, value]) => hex(decoded.txo?.[key]) !== value)) {
      throw new Error('Confirmed MSB transaction differs from the exact paid checkpoint.');
    }
    const validator = this.peer.msbClient.addressToPubKeyHex(decoded.txo.va?.toString('ascii'));
    if (!/^[0-9a-f]{64}$/.test(validator ?? '')) throw new Error('MSB proof has no valid validator identity.');
    if (au(this.peer.msbClient.getFee()) !== expectedFeeAu) throw new Error('MSB fee evidence changed.');
    return { tx: prepared.surrogate.tx, signed_length: length,
      operation_hex: hex(record.value), validator, fee_au: expectedFeeAu,
      network_id: identity.network_id, msb_bootstrap: identity.msb_bootstrap };
  }

  async reconcileSubnet(prepared, proof) {
    const previous = previousVersion(prepared);
    const committed = committedRecoveryVersion(prepared);
    const view = this.peer.base.view;
    const subnetLength = committed ? signedLength(view.core) : null;
    const identity = await this.validatePrepared(prepared, { inspectPrevious: previous, validator: proof?.validator, subnetLength });
    if (previous || committed) {
      const record = await localEntry(this.msb.state.base.view, proof?.signed_length, prepared.surrogate.tx);
      const expected = this.paymentProof(prepared, identity, proof.signed_length, record, proof.fee_au);
      if (Object.entries(expected).some(([key, value]) => proof[key] !== value)) {
        throw new Error('Previous-contract MSB proof differs from the original signed record.');
      }
      if (previous) return (await this.previousCheckpointState(prepared, proof)).checkpoint;
    }
    const read = committed ? async key => (await localEntry(view, subnetLength, key))?.value ?? null : key => this.state(key);
    const tx = prepared.surrogate.tx;
    const slot = prepared.dispatch.value.slot;
    const checkpoint = await read(`checkpoint/slot/${slot}`);
    if (checkpoint) {
      if (checkpoint.type !== 'paid_state_checkpoint' || checkpoint.tx !== tx ||
          checkpoint.snapshot_hash !== prepared.dispatch.value.snapshot_hash ||
          checkpoint.paid_by !== prepared.surrogate.address) {
        throw new Error('Canonical checkpoint conflicts with the paid MSB transaction.');
      }
      return checkpoint;
    }
    const index = await read(`tx/${tx}`);
    if (index !== null) {
      const result = await read(`txi/${index}`);
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
    if (committedRecoveryVersion(prepared)) {
      const view = this.msb.state.base.view;
      if (await localEntry(view, signedLength(view.core), prepared.surrogate.tx)) return;
      if (await this.peer.msbClient.getTxvHex() !== prepared.surrogate.txv) {
        throw new Error('Committed checkpoint validator context changed before broadcast.');
      }
    }
    return await bounded(this.peer.protocol.instance.broadcastPreparedTransaction(prepared), 'MSB checkpoint broadcast');
  }
}
