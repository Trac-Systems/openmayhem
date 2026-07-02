import b4a from 'b4a';
import { NONCE_BYTE_LENGTH, WRITER_BYTE_LENGTH } from '../../../utils/constants.js';

export const BOOTSTRAP_DEPLOYMENT_SIZE = WRITER_BYTE_LENGTH + NONCE_BYTE_LENGTH + 4; // 4 bytes for OperationType because it is a UInt32BE
export const MAXIMUM_OPERATION_PAYLOAD_SIZE = 4096; // Maximum size of a transaction buffer in bytes

// 0.03 $TNK IS THE FEE FOR EACH TRANSACTION
export const FEE = b4a.from([
    0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00,
    0x00, 0x6a, 0x94, 0xd7,
    0x4f, 0x43, 0x00, 0x00,
]);

export const Status = Object.freeze({
    SUCCESS: 0,
    FAILURE: 1,
    IGNORE: 2
});

export default {
    Status,
    BOOTSTRAP_DEPLOYMENT_SIZE,
    MAXIMUM_OPERATION_PAYLOAD_SIZE,
    FEE
};