import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import ReadyResource from 'ready-resource';
import PartialTransactionValidator from 'trac-msb/src/core/network/protocols/shared/validators/PartialTransactionValidator.js';
import { normalizeTransactionOperation } from 'trac-msb/src/utils/normalizers.js';

export const MSB_OPERATION_TYPE = Object.freeze({
    BOOTSTRAP_DEPLOYMENT: 11,
    TX: 12,
});

export class MsbClient extends ReadyResource {
    #msb
    #partialTransactionValidator

    constructor(msbInstance) {
        super();
        this.#msb = msbInstance || null;
        this.#partialTransactionValidator = null;
    }

    async _open() {
        await this.#msb.ready()
        this.#partialTransactionValidator = new PartialTransactionValidator(this.#msb.state, null, this.#msb.config)
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
        const core = this.#msb.state?.base?.view?.core ?? null;
        if (!core) throw new Error('MSB view core not available.');
        if (!Number.isSafeInteger(targetSignedLength) || targetSignedLength < 0) {
            throw new Error('Invalid MSB signed length target.');
        }
        if (!Number.isSafeInteger(pollMs) || pollMs < 1) {
            throw new Error('Invalid MSB signed length wait poll interval.');
        }
        while (core.signedLength < targetSignedLength) {
            await new Promise((resolve) => {
                const onAppend = () => {
                    cleanup();
                    resolve();
                };
                const cleanup = () => {
                    clearTimeout(timer);
                    if (typeof core.off === 'function') {
                        core.off('append', onAppend);
                    } else if (typeof core.removeListener === 'function') {
                        core.removeListener('append', onAppend);
                    }
                };
                const timer = setTimeout(() => {
                    cleanup();
                    resolve();
                }, pollMs);
                core.once('append', onAppend);
            });
        }
    }

    async getSignedAtLength(key, signedLength) {
        const viewSession = this.#msb.state.base.view.checkout(signedLength);
        try {
            return await viewSession.get(key);
        } finally {
            await viewSession.close();
        }
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
        const safePayload = this.#orchestratorCompatiblePayload(payload);
        const ok = await this.#msb.network.validatorMessageOrchestrator.send(safePayload);
        return { message: ok ? 'Transaction broadcasted successfully.' : 'Transaction broadcast failed.', tx: null };
    }

    async broadcastBootstrapDeployment(payload) {
        const safePayload = this.#orchestratorCompatiblePayload(payload);
        return await this.#msb.network.validatorMessageOrchestrator.send(safePayload);
    }
}
