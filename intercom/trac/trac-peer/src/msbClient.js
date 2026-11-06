import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import ReadyResource from 'ready-resource';
import { blake3 } from '@tracsystems/blake3';
import PartialTransactionValidator from 'trac-msb/src/core/network/protocols/shared/validators/PartialTransactionValidator.js';
import PartialBootstrapDeploymentValidator from 'trac-msb/src/core/network/protocols/shared/validators/PartialBootstrapDeploymentValidator.js';
import {
    normalizeBootstrapDeploymentOperation,
    normalizeTransactionOperation
} from 'trac-msb/src/utils/normalizers.js';
import { applyStateMessageFactory } from 'trac-msb/src/messages/state/applyStateMessageFactory.js';
import { safeEncodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import { createReadRecovery } from './network/readRecovery.js';

export const MSB_OPERATION_TYPE = Object.freeze({
    BOOTSTRAP_DEPLOYMENT: 11,
    TX: 12,
});

export class MsbClient extends ReadyResource {
    #msb
    #partialTransactionValidator
    #readSliceMs
    #onReadStall
    #log
    #now
    #confirmationStallMs
    #lastWaitRecoveryAt = -Infinity

    constructor(msbInstance, { readSliceMs = 30000, confirmationStallMs = 120000, onReadStall = null, log = console.log, now = Date.now } = {}) {
        super();
        this.#msb = msbInstance || null;
        this.#partialTransactionValidator = null;
        this.#readSliceMs = readSliceMs;
        this.#onReadStall = onReadStall;
        this.#log = log;
        this.#now = now;
        this.#confirmationStallMs = confirmationStallMs;
    }

    async _open() {
        await this.#msb.ready()
        this.#partialTransactionValidator = new PartialTransactionValidator(this.#msb.state, null, this.#msb.config)
        const swarm = this.#msb.network?.swarm;
        const topic = this.#msb.config?.channel;
        if (this.#onReadStall === null && typeof this.#msb.network?.recoverRead === 'function') {
            // All MSB consumers, including its standalone confirmation watch,
            // share the network's recovery cooldown and in-flight discovery.
            this.#onReadStall = event => this.#msb.network.recoverRead(event);
        } else if (this.#onReadStall === null && swarm && topic) {
            this.#onReadStall = createReadRecovery({
                base: this.#msb.state.base, swarm, topic, label: 'MSB', rotate: false,
                now: this.#now, log: this.#log,
            });
        }
    }

    recoverRead(event) {
        try {
            const result = this.#onReadStall?.(event) ?? false;
            result?.catch?.(() => {});
            return result;
        }
        catch { return false; }
    }

    #report(event) {
        try { this.#log(`[trac-peer] ${JSON.stringify({ chain: 'MSB', ...event })}`); }
        catch {}
    }

    #orchestratorCompatiblePayload(payload) {
        if (!payload || typeof payload !== 'object') return payload;
        if (payload.tro && payload.tro.tx) return payload;
        const tx =
            payload?.tro?.tx ??
            payload?.txo?.tx ??
            payload?.bdo?.tx ??
            payload?.rao?.tx ??
            null;
        if (!tx) return payload;
        return { ...payload, tro: { ...(payload.tro || {}), tx } };
    }

    get addressPrefix() {
        return this.#msb.config.addressPrefix
    }

    get derivationPath() {
        return this.#msb.config.derivationPath
    }

    get networkId() {
        return this.#msb.config.networkId
    }

    get bootstrapHex() {
        const buf = this.#msb.config.bootstrap
        return b4a.isBuffer(buf) ? buf.toString('hex') : null;
    }

    async getTxvHex() {
        const txv = await this.#msb.state.getIndexerSequenceState();
        return txv.toString('hex');
    }

    pubKeyHexToAddress(pubKeyHex) {
        return PeerWallet.encodeBech32mSafe(this.addressPrefix, b4a.from(pubKeyHex, 'hex'));
    }

    addressToPubKeyHex(address) {
        const decoded = PeerWallet.decodeBech32mSafe(address);
        return b4a.toString(decoded, 'hex');
    }

    getSignedLength() {
        return this.#msb.state.getSignedLength();
    }

    getUnsignedLength() {
        return this.#msb.state.getUnsignedLength();
    }

    getFee() {
        if (typeof this.#msb.state.getFee !== 'function') return null;
        return this.#msb.state.getFee();
    }

    async getNodeEntryUnsigned(address) {
        return await this.#msb.state.getNodeEntryUnsigned(address);
    }

    getConnectedValidatorsCount() {
        try {
            return this.#msb.network?.validatorConnectionManager?.connectionCount?.() ?? 0;
        } catch (_e) {
            return 0;
        }
    }

    async tryConnect(pubKeyHex, role = 'validator') {
        return await this.#msb.network.tryConnect(pubKeyHex, role);
    }

    async waitForSignedLengthAtLeast(targetSignedLength, { pollMs = 1_000 } = {}) {
        const base = this.#msb.state?.base;
        const core = base?.view?.core ?? null;
        if (!core) throw new Error('MSB view core not available.');
        if (!Number.isSafeInteger(targetSignedLength) || targetSignedLength < 0) {
            throw new Error('Invalid MSB signed length target.');
        }
        if (!Number.isSafeInteger(pollMs) || pollMs < 1) {
            throw new Error('Invalid MSB signed length wait poll interval.');
        }
        const started = this.#now();
        let confirmed = core.signedLength, progressedAt = started;
        const observe = () => {
            const current = base.view.core;
            const now = this.#now();
            if (current.signedLength !== confirmed) {
                confirmed = current.signedLength;
                progressedAt = now;
            }
            return { current, now };
        };
        const reporter = setInterval(() => {
            const { current, now } = observe();
            if (base.closing || base.closed || this.closing || this.closed || current.signedLength >= targetSignedLength) return;
            const event = { code: 'MSB_CONFIRMATION_STALLED', event: 'msb_wait_signed_length',
                msbsl: targetSignedLength, signedLength: current.signedLength,
                elapsedMs: Math.max(0, now - started), stalledMs: Math.max(0, now - progressedAt) };
            this.#report(event);
            if (Number.isFinite(this.#confirmationStallMs) && this.#confirmationStallMs > 0 &&
                event.stalledMs >= this.#confirmationStallMs && now - this.#lastWaitRecoveryAt >= this.#readSliceMs) {
                this.#lastWaitRecoveryAt = now;
                this.recoverRead(event);
            }
        }, this.#readSliceMs);
        reporter.unref?.();
        try {
            while (base.view.core.signedLength < targetSignedLength) {
                const { current } = observe();
                if (base.closing || base.closed || this.closing || this.closed || current.closed) {
                    throw Object.assign(new Error('MSB closed while waiting for confirmation.'), { code: 'SESSION_CLOSED' });
                }
                await new Promise((resolve) => {
                    const onAppend = () => {
                        cleanup();
                        resolve();
                    };
                    const cleanup = () => {
                        clearTimeout(timer);
                        current.removeListener('append', onAppend);
                        current.removeListener('close', onAppend);
                    };
                    const timer = setTimeout(() => {
                        cleanup();
                        resolve();
                    }, pollMs);
                    current.once('append', onAppend);
                    current.once('close', onAppend);
                });
            }
        } finally { clearInterval(reporter); }
    }

    async getSignedAtLength(key, signedLength) {
        const started = this.#now();
        for (let attempt = 1; ; attempt++) {
            let waitingSeq = null;
            const base = this.#msb.state.base;
            const viewSession = base.view.checkout(signedLength);
            try {
                return await viewSession.get(key, {
                    timeout: this.#readSliceMs,
                    onwait: seq => { waitingSeq = seq; },
                    onseq: () => { waitingSeq = null; },
                });
            } catch (error) {
                if (error?.code !== 'REQUEST_TIMEOUT' || base.closing || base.closed) throw error;
                const core = base.view.core;
                const event = {
                    code: 'MSB_APPLY_READ_STALLED', event: 'msb_read_stalled', key, msbsl: signedLength,
                    attempt, waitingSeq, signedLength: core?.signedLength ?? null,
                    corePeers: core?.peers?.length ?? 0,
                    coreKey: core?.key ? b4a.toString(core.key, 'hex') : null,
                    elapsedMs: Math.max(0, this.#now() - started),
                };
                this.#report(event);
                try { this.emit('read-stall', event); } catch {}
                this.recoverRead(event);
            } finally {
                // Closing a checkout must not replace the read's result or error.
                try { await viewSession.close(); } catch {}
            }
        }
    }

    async getUnexecutedContextChange(txHex, preparedTxv) {
        // A live/unsigned validator sequence alone cannot retire an uncertain
        // payment. Bind non-execution to the view named by a SIGNED system state
        // that has already moved beyond the transaction's validator context.
        const base = this.#msb.state.base;
        const system = base.system;
        const systemLength = system?.core?.signedLength;
        if (!Number.isSafeInteger(systemLength) || systemLength < 1) return null;
        const info = await system.getIndexedInfo(systemLength);
        if (!Array.isArray(info?.indexers) || info.indexers.length === 0) return null;
        const txv = b4a.toString(await blake3(b4a.concat(info.indexers.map((entry) => entry.key))), 'hex');
        if (txv === preparedTxv || txv !== await this.getTxvHex()) return null;
        const viewKey = b4a.toString(base.view.core.key, 'hex');
        const linked = info.views?.find((view) => b4a.toString(view.key, 'hex') === viewKey);
        if (!linked || !Number.isSafeInteger(linked.length) || linked.length < 1 ||
            linked.length > this.getSignedLength()) return null;
        if (await this.getSignedAtLength(txHex, linked.length) !== null) return null;
        // Recheck the latest signed view too. A payment observed at either
        // boundary must be reconciled, never replaced with a fresh fee.
        const currentLength = this.getSignedLength();
        if (await this.getSignedAtLength(txHex, currentLength) !== null) return null;
        return { type: 'unexecuted_at_signed_context_change', tx: txHex,
            previous_txv: preparedTxv, txv,
            system_key: b4a.toString(system.core.key, 'hex'), system_signed_length: systemLength,
            view_key: viewKey, linked_view_signed_length: linked.length,
            checked_view_signed_length: currentLength };
    }

    async validateTransaction(payload) {
        try {
            const normalized = normalizeTransactionOperation(payload, this.#msb.config);
            await this.#partialTransactionValidator.validate(normalized);
            return true;
        } catch (e) {
            const msg = typeof e?.message === 'string' ? e.message : 'MSB transaction validation failed.';
            throw new Error(`Invalid MSB tx: ${msg}`);
        }
    }

    async broadcastTransaction(payload) {
        if (this.#msb.state?.isWritable?.() === true) {
            const normalized = normalizeTransactionOperation(payload, this.#msb.config);
            await this.#partialTransactionValidator.validate(normalized);
            const txo = normalized.txo;
            const txHex = b4a.toString(txo.tx, 'hex');
            const complete = await applyStateMessageFactory(
                this.#msb.wallet,
                this.#msb.config
            ).buildCompleteTransactionOperationMessage(
                normalized.address,
                txo.tx,
                txo.txv,
                txo.iw,
                txo.in,
                txo.ch,
                txo.is,
                txo.bs,
                txo.mbs
            );
            await this.#msb.state.append(safeEncodeApplyOperation(complete));
            if (typeof this.#msb.state.base?.forceFastForward === 'function') {
                await this.#msb.state.base.forceFastForward();
            }
            const deadline =
                Date.now() + (this.#msb.config.messageValidatorResponseTimeout ?? 30_000);
            let signedEntry = await this.#msb.state.getSigned(txHex);
            while (!signedEntry && Date.now() < deadline) {
                await new Promise((resolve) => setTimeout(resolve, 50));
                signedEntry = await this.#msb.state.getSigned(txHex);
            }
            if (!signedEntry) {
                throw new Error('Local MSB transaction did not become signed before timeout.');
            }
            const validatorPubKey = b4a.isBuffer(this.#msb.wallet.publicKey)
                ? b4a.toString(this.#msb.wallet.publicKey, 'hex')
                : String(this.#msb.wallet.publicKey ?? '').toLowerCase();
            if (!/^[0-9a-f]{64}$/.test(validatorPubKey)) {
                throw new Error('Local MSB validator public key is not hex.');
            }
            return {
                message: 'Transaction broadcasted successfully.',
                tx: null,
                localCommit: {
                    msbsl: this.#msb.state.getSignedLength(),
                    validator: validatorPubKey,
                },
            };
        }
        const safePayload = this.#orchestratorCompatiblePayload(payload);
        const ok = await this.#msb.network.validatorMessageOrchestrator.send(safePayload);
        return { message: ok ? 'Transaction broadcasted successfully.' : 'Transaction broadcast failed.', tx: null };
    }

    async broadcastBootstrapDeployment(payload) {
        if (this.#msb.state?.isWritable?.() === true) {
            const normalized = normalizeBootstrapDeploymentOperation(payload, this.#msb.config);
            await new PartialBootstrapDeploymentValidator(
                this.#msb.state,
                null,
                this.#msb.config
            ).validate(normalized);
            const bdo = normalized.bdo;
            const complete = await applyStateMessageFactory(
                this.#msb.wallet,
                this.#msb.config
            ).buildCompleteBootstrapDeploymentMessage(
                normalized.address,
                bdo.tx,
                bdo.txv,
                bdo.bs,
                bdo.ic,
                bdo.in,
                bdo.is
            );
            await this.#msb.state.append(safeEncodeApplyOperation(complete));
            if (typeof this.#msb.state.base?.forceFastForward === 'function') {
                await this.#msb.state.base.forceFastForward();
            }
            const bootstrapHex = b4a.toString(bdo.bs, 'hex');
            const deadline =
                Date.now() + (this.#msb.config.messageValidatorResponseTimeout ?? 30_000);
            let deployment = await this.#msb.state.getRegisteredBootstrapEntry(bootstrapHex);
            while (!deployment && Date.now() < deadline) {
                await new Promise((resolve) => setTimeout(resolve, 50));
                deployment = await this.#msb.state.getRegisteredBootstrapEntry(bootstrapHex);
            }
            if (!deployment) {
                throw new Error('Local MSB bootstrap deployment did not become signed before timeout.');
            }
            return true;
        }
        const safePayload = this.#orchestratorCompatiblePayload(payload);
        return await this.#msb.network.validatorMessageOrchestrator.send(safePayload);
    }
}
