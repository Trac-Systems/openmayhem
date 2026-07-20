import b4a from 'b4a';
import PeerWallet from 'trac-wallet';
import ReadyResource from 'ready-resource';

export const MSB_OPERATION_TYPE = Object.freeze({
    BOOTSTRAP_DEPLOYMENT: 11,
    TX: 12,
});

export class MsbClient extends ReadyResource {
    #msb

    constructor(msbInstance) {
        super();
        this.#msb = msbInstance || null;
    }

    async _open() {
        return await this.#msb.ready()
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
        if (typeof this.#msb.state.getUnsignedLength !== 'function') return null;
        return this.#msb.state.getUnsignedLength();
    }

    getFee() {
        if (typeof this.#msb.state.getFee !== 'function') return null;
        return this.#msb.state.getFee();
    }

    async getNodeEntryUnsigned(address) {
        if (typeof this.#msb.state.getNodeEntryUnsigned !== 'function') return null;
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
        if (typeof this.#msb.network?.tryConnect !== 'function') {
            throw new Error('MSB network does not support tryConnect.');
        }
        return await this.#msb.network.tryConnect(pubKeyHex, role);
    }

    async waitForSignedLengthAtLeast(targetSignedLength) {
        const core = this.#msb.state?.base?.view?.core ?? null;
        if (!core) throw new Error('MSB view core not available.');
        while (core.signedLength < targetSignedLength) {
            await new Promise((resolve) => core.once('append', resolve));
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

    async broadcastTransaction(payload) {
        const safePayload = this.#orchestratorCompatiblePayload(payload);
        if (typeof this.#msb.broadcastTransactionCommand === 'function') {
            return await this.#msb.broadcastTransactionCommand(safePayload);
        }
        if (this.#msb.network?.validatorMessageOrchestrator?.send) {
            const ok = await this.#msb.network.validatorMessageOrchestrator.send(safePayload);
            return { message: ok ? 'Transaction broadcasted successfully.' : 'Transaction broadcast failed.', tx: null };
        }
        throw new Error('MSB does not support transaction broadcasting.');
    }

    async broadcastBootstrapDeployment(payload) {
        const safePayload = this.#orchestratorCompatiblePayload(payload);
        if (this.#msb.network?.validatorMessageOrchestrator?.send) {
            return await this.#msb.network.validatorMessageOrchestrator.send(safePayload);
        }
        if (typeof this.#msb.broadcastPartialTransaction === 'function') {
            return await this.#msb.broadcastPartialTransaction(safePayload);
        }
        throw new Error('MSB does not support bootstrap deployment broadcasting.');
    }
}
