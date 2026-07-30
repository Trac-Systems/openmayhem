import {ResultCode} from '../../../../utils/constants.js';

/** 
 * V1 protocol error type.
 *
 * `V1ProtocolError` is the v1 base class used by handlers/validators to attach:
 * - `resultCode`: a stable `ResultCode` enum value for programmatic handling
 */
export class V1ProtocolError extends Error {
    /**
     * @param {number} resultCode Stable rejection reason (a `ResultCode` enum value).
     * @param {string} message Human-readable error message.
     */
    constructor(resultCode, message) {
        super(message);
        this.name = this.constructor.name;
        this.resultCode = resultCode;
    }
}

export function getResultCode(err) {
    return err instanceof V1ProtocolError ? err.resultCode : ResultCode.UNEXPECTED_ERROR;
}

export function shouldEndConnection(err) {
    return err instanceof V1ProtocolError ? Boolean(err.endConnection) : false;
}
