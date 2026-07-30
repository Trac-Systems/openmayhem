import { NETWORK_MESSAGE_TYPES } from '../../../../../utils/constants.js';
import tracCryptoApi from 'trac-crypto-api';
import b4a from 'b4a';

class LegacyGetRequestHandler {
    #wallet;
    #state;

    constructor(wallet, state) {
        this.#wallet = wallet;
        this.#state = state;
    }

    get state() {
        return this.#state;
    }

    async handle(message, connection, channelString) {
        switch (message) {
            case NETWORK_MESSAGE_TYPES.GET.VALIDATOR:
                await this.handleGetValidatorResponse(connection, channelString);
                break;
            default:
                throw new Error(`Unhandled GET type: ${message}`);
        }
    }

    async handleGetValidatorResponse(connection, channelString) {
        const nonce = b4a.toString(tracCryptoApi.nonce.generate(), 'hex');
        const payload = {
            op: 'validatorResponse',
            wk: this.state.writingKey.toString('hex'),
            address: this.#wallet.address,
            nonce: nonce,
            channel: channelString,
            issuer: connection.remotePublicKey.toString('hex'),
            timestamp: Date.now(),
        };


        const hashInput = b4a.from(JSON.stringify(payload), 'utf8');
        const hash = await tracCryptoApi.hash.blake3(hashInput);
        const sig = this.#wallet.sign(hash);

        const responseMessage = {
            ...payload,
            sig: sig.toString('hex'),
        };
        connection.protocolSession.sendAndForget(responseMessage)
    }
}

export default LegacyGetRequestHandler;
