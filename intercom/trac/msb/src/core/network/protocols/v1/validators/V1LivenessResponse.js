import V1BaseOperation from "./V1BaseOperation.js";

class V1LivenessResponse extends V1BaseOperation {
    constructor(config) {
        super(config);
    }

    async validate(payload, connection, pendingRequestServiceEntry) {
        this.isPayloadSchemaValid(payload);
        this.validateResponseType(payload, pendingRequestServiceEntry);
        this.validatePeerCorrectness(connection.remotePublicKey, pendingRequestServiceEntry);
        await this.validateSignature(payload, connection.remotePublicKey);
        return true;
    }
}

export default V1LivenessResponse;
