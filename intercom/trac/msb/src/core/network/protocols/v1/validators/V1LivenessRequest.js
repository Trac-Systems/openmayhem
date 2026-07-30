import V1BaseOperation from "./V1BaseOperation.js";

class V1LivenessRequest extends V1BaseOperation {
    constructor(config) {
        super(config);
    }

    async validate(payload, remotePublicKey) {
        this.isPayloadSchemaValid(payload);
        await this.validateSignature(payload, remotePublicKey);
        return true;
    }
}

export default V1LivenessRequest;
