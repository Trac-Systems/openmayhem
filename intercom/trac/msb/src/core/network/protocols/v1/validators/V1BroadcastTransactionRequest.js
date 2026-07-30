import V1BaseOperation from "./V1BaseOperation.js";
import b4a from "b4a";
import {MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE, ResultCode} from '../../../../../utils/constants.js';
import {V1ProtocolError} from "../V1ProtocolError.js";

class V1BroadcastTransactionRequest extends V1BaseOperation {
    constructor(config) {
        super(config);
    }

    async validate(payload, remotePublicKey) {
        this.isPayloadSchemaValid(payload);
        this.isDataPropertySizeValid(payload);
        await this.validateSignature(payload, remotePublicKey);
        return true;
    }

    isDataPropertySizeValid(payload) {
        if (b4a.byteLength(payload.broadcast_transaction_request.data) > MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE) {
            throw new V1ProtocolError(
                ResultCode.INVALID_PAYLOAD,
                `The 'data' field exceeds the maximum allowed byte size of ${MAX_PARTIAL_TX_PAYLOAD_BYTE_SIZE}. Actual size: ${b4a.byteLength(payload.broadcast_transaction_request.data)}`);
        }
    }

}

export default V1BroadcastTransactionRequest;
