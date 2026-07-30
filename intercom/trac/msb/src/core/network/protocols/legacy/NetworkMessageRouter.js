import b4a from "b4a";
import _ from 'lodash';
import LegacyGetRequestHandler from "./handlers/LegacyGetRequestHandler.js";
import LegacyResponseHandler from "./handlers/LegacyResponseHandler.js";
import LegacyRoleOperationHandler from "./handlers/LegacyRoleOperationHandler.js";
import LegacySubnetworkOperationHandler from "./handlers/LegacySubnetworkOperationHandler.js";
import LegacyTransferOperationHandler from "./handlers/LegacyTransferOperationHandler.js";
import { NETWORK_MESSAGE_TYPES } from '../../../../utils/constants.js';
import * as operation from '../../../../utils/applyOperations.js';

class NetworkMessageRouter {
    #handlers;
    #config;

    /**
     * @param {State} state
     * @param {IWallet} wallet
     * @param {TransactionRateLimiterService} rateLimiterService
     * @param {TransactionPoolService} txPoolService
     * @param {Config} config
     **/
    constructor(state, wallet, rateLimiterService, txPoolService, config) {
        this.#config = config;

        this.#handlers = {
            get: new LegacyGetRequestHandler(wallet, state),
            response: new LegacyResponseHandler(state, wallet, this.#config),
            roleTransaction: new LegacyRoleOperationHandler(state, wallet, rateLimiterService, txPoolService, this.#config),
            subNetworkTransaction: new LegacySubnetworkOperationHandler(state, wallet, rateLimiterService, txPoolService, this.#config),
            tracNetworkTransaction: new LegacyTransferOperationHandler(state, wallet, rateLimiterService, txPoolService, this.#config),

        }
    }

    async route(incomingMessage, connection) {
        this.#preValidate(incomingMessage);
        const channelString = b4a.toString(this.#config.channel, 'utf8');

        // We received a legacy message, so we set the connection protocol accordingly
        connection.protocolSession.setLegacyAsPreferredProtocol();
        if (this.#isGetRequest(incomingMessage)) {
            await this.#handlers.get.handle(incomingMessage, connection, channelString);
        }
        else if (this.#isResponse(incomingMessage)) {
            await this.#handlers.response.handle(incomingMessage, channelString);
        }
        else if (this.#isRoleAccessOperation(incomingMessage)) {
            await this.#handlers.roleTransaction.handle(incomingMessage, connection);
        }
        else if (this.#isSubnetworkOperation(incomingMessage)) {
            await this.#handlers.subNetworkTransaction.handle(incomingMessage, connection);
        }
        else if (this.#isTransferOperation(incomingMessage)) {
            await this.#handlers.tracNetworkTransaction.handle(incomingMessage, connection);
        }
        else {
            throw new Error(`Failed to route message. Pubkey of requester is ${connection.remotePublicKey ? b4a.toString(connection.remotePublicKey, 'hex') : 'unknown'}`);
        }
    }

    #preValidate(message) {
        if (!_.isPlainObject(message) && typeof message !== 'string') {
            throw new Error('Invalid message format: expected object or string.');
        }
    }

    #isGetRequest(message) {
        return Object.values(NETWORK_MESSAGE_TYPES.GET).includes(message);
    }

    #isResponse(message) {
        return Object.values(NETWORK_MESSAGE_TYPES.RESPONSE).includes(message.op);
    }

    #isRoleAccessOperation(message) {
        return operation.isRoleAccess(message.type)
    }

    #isSubnetworkOperation(message) {
        return operation.isTransaction(message.type) ||
            operation.isBootstrapDeployment(message.type)
    }

    #isTransferOperation(message) {
        return operation.isTransfer(message.type)
    }
}


export default NetworkMessageRouter;
