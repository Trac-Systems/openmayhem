import {ResultCode} from "../../../../../utils/constants.js";
import {publicKeyToAddress} from "../../../../../utils/helpers.js";
import {V1ProtocolError} from "../V1ProtocolError.js";

class V1BaseOperationHandler {
    #rateLimiterService;
    #pendingRequestService;
    #config;

    constructor(rateLimiterService, pendingRequestService, config) {
        this.#rateLimiterService = rateLimiterService;
        this.#pendingRequestService = pendingRequestService;
        this.#config = config;
    }

    get config() {
        return this.#config;
    }

    applyRateLimit(connection) {
        if (!this.#config.disableRateLimit) {
            this.#rateLimiterService.v1HandleRateLimit(connection);
        }
    }

    async resolvePendingResponse(message, connection, validator, extractResultCode) {
        const pendingRequestServiceEntry = this.#pendingRequestService.getPendingRequest(message.id);
        if (!pendingRequestServiceEntry) return false;

        this.#pendingRequestService.stopPendingRequestTimeout(message.id);
        await validator.validate(message, connection, pendingRequestServiceEntry);

        const resultCode = extractResultCode(message);
        this.#pendingRequestService.resolvePendingRequest(message.id, resultCode);
        return true;
    }

    handlePendingResponseError(messageId, connection, error, step) {
        const protocolError = this.#toProtocolError(error);
        const rejected = this.#pendingRequestService.rejectPendingRequest(messageId, protocolError);
        if (!rejected) return;
        this.displayError(step, connection.remotePublicKey, protocolError);
    }

    async sendResponseAndMaybeClose(connection, response, endConnection) {
        connection.protocolSession.sendAndForget(response);
        if (!endConnection) return;

        await connection.flush();
        connection.end();
    }

    displayError(step = "undefined step", senderPublicKey, error) {
        const errorMessage = error?.message ?? 'Unexpected error';
        console.error(`${this.constructor.name}: ${step} ${publicKeyToAddress(senderPublicKey, this.#config)}: ${errorMessage}`);
    }

    #toProtocolError(error) {
        if (error instanceof V1ProtocolError) {
            return error;
        }
        return new V1ProtocolError(ResultCode.UNEXPECTED_ERROR, error?.message ?? 'Unexpected error');
    }
}

export default V1BaseOperationHandler;
