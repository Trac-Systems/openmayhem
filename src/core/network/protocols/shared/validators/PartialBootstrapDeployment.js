import PartialOperation from './base/PartialOperation.js';

class PartialBootstrapDeployment extends PartialOperation {
    constructor(state, selfAddress , config) {
        super(state, selfAddress , config);
    }

    async validate(payload) {
        this.isPayloadSchemaValid(payload);
        this.validateNoSelfValidation(payload);
        this.validateRequesterAddress(payload);
        await this.validateTransactionUniqueness(payload);
        await this.validateSignature(payload);
        await this.validateTransactionValidity(payload);
        this.isOperationNotCompleted(payload);
        await this.validateRequesterBalance(payload);
        await this.validateRequesterBalance(payload, true);
        this.validateSubnetworkBootstrapEquality(payload);

        // non common validations below
        await this.validateBootstrapRegistration(payload)

        return true;
    }

    async validateBootstrapRegistration(payload) {
        const bootstrapString = payload.bdo.bs.toString('hex');
        if (null !== await this.state.getRegisteredBootstrapEntryUnsigned(bootstrapString)) {
            throw new Error(`Bootstrap with hash ${bootstrapString} already exists in the state. Bootstrap must be unique.`);
        }
    }
}

export default PartialBootstrapDeployment;
