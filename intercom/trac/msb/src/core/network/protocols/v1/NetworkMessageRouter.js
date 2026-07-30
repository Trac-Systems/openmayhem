import { decodeV1networkOperation } from '../../../../utils/protobuf/operationHelpers.js'
import b4a from 'b4a'
import { NetworkOperationType, V1_PROTOCOL_PAYLOAD_MAX_SIZE } from '../../../../utils/constants.js'
import { publicKeyToAddress } from '../../../../utils/helpers.js'
import V1LivenessOperationHandler from './handlers/V1LivenessOperationHandler.js'
import V1BroadcastTransactionOperationHandler from './handlers/V1BroadcastTransactionOperationHandler.js'

class NetworkMessageRouterV1 {
    #config
    #livenessRequestHandler
    #broadcastTransactionHandler

    constructor(
        state,
        wallet,
        rateLimiterService,
        txPoolService,
        pendingRequestsService,
        transactionCommitService,
        config
    ) {
        this.#config = config
        this.#livenessRequestHandler = new V1LivenessOperationHandler(
            wallet,
            rateLimiterService,
            pendingRequestsService,
            config
        );
        this.#broadcastTransactionHandler = new V1BroadcastTransactionOperationHandler(
            state,
            wallet,
            rateLimiterService,
            txPoolService,
            pendingRequestsService,
            transactionCommitService,
            config
        );
    }

    async route(incomingMessage, connection) {
        if (!this.#preValidate(incomingMessage)) {
            this.#disconnect(connection, 'Pre-validation failed for incoming V1 message')
            return;
        }
        let decodedMessage;

        try {
            decodedMessage = decodeV1networkOperation(incomingMessage)
        } catch (error) {
            this.#disconnect(connection, `Failed to decode incoming V1 message: ${error.message}`)
            return;
        }

        // TODO: Decide if we really need to check decodedMessage.type here, since this is done
        // again in the next switch statement
        if (!decodedMessage || !Number.isInteger(decodedMessage.type) || decodedMessage.type <= 0) {
            this.#disconnect(connection, `Invalid V1 message type: ${decodedMessage?.type}`)
            return;
        }

        // We received a v1 message, so we set the connection protocol accordingly
        connection.protocolSession.setV1AsPreferredProtocol()

        try {
            switch (decodedMessage.type) {
                case NetworkOperationType.LIVENESS_REQUEST:
                    await this.#livenessRequestHandler.handleRequest(decodedMessage, connection);
                    break;
                case NetworkOperationType.LIVENESS_RESPONSE:
                    await this.#livenessRequestHandler.handleResponse(decodedMessage, connection);
                    break;

                case NetworkOperationType.BROADCAST_TRANSACTION_REQUEST:
                    await this.#broadcastTransactionHandler.handleRequest(decodedMessage, connection);
                    break;

                case NetworkOperationType.BROADCAST_TRANSACTION_RESPONSE:
                    await this.#broadcastTransactionHandler.handleResponse(decodedMessage, connection);
                    break;
                default:
                    this.#disconnect(connection, `Unsupported V1 message type: ${decodedMessage.type}`)
            }
        } catch (error) {
            this.#disconnect(connection, `Unhandled error while routing V1 message: ${error.message}`)
        }
    }

    #preValidate(incomingMessage) {
        return !(!incomingMessage || !b4a.isBuffer(incomingMessage) || incomingMessage.length === 0 || incomingMessage.length > V1_PROTOCOL_PAYLOAD_MAX_SIZE);
    }

    #disconnect(connection, reason) {
        const sender = publicKeyToAddress(connection.remotePublicKey, this.#config)
        console.error(`NetworkMessageRouterV1: ${reason}, sender: ${sender}`)
        connection.end();
    }
}

export default NetworkMessageRouterV1
