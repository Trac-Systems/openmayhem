import b4a from 'b4a';
import BaseResponse from './base/BaseResponse.js';
class ValidatorResponse extends BaseResponse {

    constructor(state, wallet, config) {
        super(state, wallet, config);
    }

    async validate(message, channelString) {
        if (
            !this.validatePayload(message) ||
            !this.validateIssuerPublicKey(message) ||
            !this.validateTimestamp(message) ||
            !await this.validateNodeEntry(message) ||
            //!await this.validateWritingKey(message) || DISABLED BECAUSE WHEN VALIDATOR WANT TO REGISTER NEW WK, THIS IS NOT POSSIBLE
            !await this.validateSignature(message) ||
            !this.validateChannel(message, channelString)
        ) {
            throw new Error("Validator response validation failed");
        }
        return true;
    }

    validatePayload(message) {
        if (!message ||
            !message.op ||
            !message.wk ||
            !message.address ||
            !message.nonce ||
            !message.channel ||
            !message.issuer ||
            !message.timestamp) {
            throw new Error("Validator response is missing required fields.");
        }
        return true;
    }

    async validateNodeEntry(message) {
        const validatorEntry = await this.state.getNodeEntry(message.address);
        const adminEntry = await this.state.getAdminEntry();

        if (validatorEntry === null || !validatorEntry.isWriter) {
            throw new Error("Validator entry not found in state or is not a writer.");
        }

        if (adminEntry && message.address === adminEntry.address) {
            return true;
        }

        if (validatorEntry.isIndexer) {
            throw new Error("Validator is an indexer.");
        }
        return true;
    }

    async validateWritingKey(message) {
        const writingKey = b4a.from(message.wk, 'hex');
        const validatorEntry = await this.state.getNodeEntry(message.address);
        
        if (!validatorEntry || !b4a.equals(validatorEntry.wk, writingKey)) {
            throw new Error("Writing key does not match validator entry.");
        }
        return true;
    }
}

export default ValidatorResponse;
