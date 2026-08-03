import b4a from 'b4a';
import tracCryptoApi from 'trac-crypto-api';

/*
    BaseResponse class for handling common validation logic for network responses.
*/

class BaseResponse {
    #wallet;
    #state;

    /**
     * 
     * @param {State} state 
     * @param {IWallet} wallet
     * @param {Config} config
     */
    constructor(state, wallet, _config) {
        this.#state = state;
        this.#wallet = wallet;
    }

    get state() {
        return this.#state;
    }

    validateIssuerPublicKey(message) {
        const issuerPublicKey = b4a.from(message.issuer, 'hex');
        if (!b4a.equals(issuerPublicKey, this.#wallet.publicKey)) {
            throw new Error("Issuer public key does not match the wallet's public key.");
        }
        return true;
    }

    validateTimestamp(message) {
        const timestamp = message.timestamp;
        const now = Date.now();
        const treshhold = 10000;

        if (now - timestamp > treshhold) {
            throw new Error("Validator response is too old, ignoring.");
        }
        return true;
    }

    validateChannel(message, channelString) {
        if (message.channel !== channelString) {
            throw new Error("Channel mismatch in validator response.");
        }
        return true;
    }

    /*
        Signature validation methods (validateAdminSignature, validateValidatorSignature, validateCustomNodeSignature)
        are implemented in this base class due to JavaScript encapsulation limitations.
        JS does not have a 'protected' modifier, and we want to keep the wallet field private (#wallet)
        and prevent access to it from derived classes or external code.
        Therefore, all logic requiring access to the wallet must be implemented here.
    */

    async validateSignature(message, type = null) {
        let publicKey = null;
        switch (type) {
            case 'admin':
                const adminEntry = await this.state.getAdminEntry();
                publicKey = tracCryptoApi.address.decode(adminEntry.address);

                break;
            default:
                publicKey = tracCryptoApi.address.decode(message.address);
        }

        if (!publicKey) {
            throw new Error("Failed to derive public key from message.");
        }

        const messageWithoutSig = { ...message };
        delete messageWithoutSig.sig;
        const hashInput = b4a.from(JSON.stringify(messageWithoutSig), 'utf8');
        const hash = await tracCryptoApi.hash.blake3(hashInput);
        const signature = b4a.from(message.sig, 'hex');
        const verified = this.#wallet.verify(signature, hash, publicKey);

        if (!verified) {
            throw new Error("Signature in the response verification failed.");
        }
        return true;
    }
}

export default BaseResponse;
