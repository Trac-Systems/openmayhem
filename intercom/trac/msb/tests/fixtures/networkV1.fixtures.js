import b4a from 'b4a';
import { v7 as uuidv7 } from 'uuid';
import { NetworkOperationType, ResultCode as NetworkResultCode } from '../../src/utils/constants.js';

const payloadLivenessRequest = {
    type: NetworkOperationType.LIVENESS_REQUEST,
    id: uuidv7(),
    timestamp: 789,
    liveness_request: {
        nonce: b4a.from('04', 'hex'),
        signature: b4a.from('05', 'hex')
    },
    capabilities: []
};

const payloadLivenessResponse = {
    type: NetworkOperationType.LIVENESS_RESPONSE,
    id: uuidv7(),
    timestamp: 101112,
    liveness_response: {
        nonce: b4a.from('06', 'hex'),
        signature: b4a.from('07', 'hex'),
        result: NetworkResultCode.OK
    },
    capabilities: []
};

const payloadBroadcastTransactionRequest = {
    type: NetworkOperationType.BROADCAST_TRANSACTION_REQUEST,
    id: uuidv7(),
    timestamp: 131415,
    broadcast_transaction_request: {
        data: b4a.from('deadbeef', 'hex'),
        nonce: b4a.from('08', 'hex'),
        signature: b4a.from('09', 'hex')
    },
    capabilities: ['cap:a']
};

const payloadBroadcastTransactionResponse = {
    type: NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE,
    id: uuidv7(),
    timestamp: 161718,
    broadcast_transaction_response: {
        nonce: b4a.from('0a', 'hex'),
        signature: b4a.from('0b', 'hex'),
        proof: b4a.from('0c', 'hex'),
        timestamp: 161719,
        result: NetworkResultCode.OK
    },
    capabilities: ['cap:b']
};

export default {
    payloadLivenessRequest,
    payloadLivenessResponse,
    payloadBroadcastTransactionRequest,
    payloadBroadcastTransactionResponse,
};
