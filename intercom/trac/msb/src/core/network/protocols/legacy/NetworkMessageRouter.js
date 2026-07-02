import b4a from "b4a";
import GetRequestHandler from "./handlers/GetRequestHandler.js";
import ResponseHandler from "./handlers/ResponseHandler.js";
import RoleOperationHandler from "../shared/handlers/RoleOperationHandler.js";
import SubnetworkOperationHandler from "../shared/handlers/SubnetworkOperationHandler.js";
import TransferOperationHandler from "../shared/handlers/TransferOperationHandler.js";
import { NETWORK_MESSAGE_TYPES } from '../../../../utils/constants.js';
import * as operation from '../../../../utils/applyOperations.js';
import State from "../../../state/State.js";
import PeerWallet from "trac-wallet";

class NetworkMessageRouter {
    #handlers;
    #config;

    /**
     * @param {State} state
     * @param {PeerWallet} wallet
     * @param {TransactionRateLimiterService} rateLimiterService
     * @param {TransactionPoolService} txPoolService
     * @param {ConnectionManager} connectionManager
     * @param {object} config
     **/
    constructor(state, wallet, rateLimiterService, txPoolService, connectionManager, config) {
        this.#config = config;

        this.#handlers = {
            get: new GetRequestHandler(wallet, state),
            response: new ResponseHandler(state, wallet, connectionManager, this.#config),
            roleTransaction: new RoleOperationHandler(state, wallet, rateLimiterService, txPoolService, this.#config),
            subNetworkTransaction: new SubnetworkOperationHandler(state, wallet, rateLimiterService, txPoolService, this.#config),
            tracNetworkTransaction: new TransferOperationHandler(state, wallet, rateLimiterService, txPoolService, this.#config),
        }
    }

    // NOTE: messageProtomux can be deleted, ad this is a session, and we can extract this from connection
    async route(incomingMessage, connection, messageProtomux) {
        const channelString = b4a.toString(this.#config.channel, 'utf8');
        if (this.#isGetRequest(incomingMessage)) {
            await this.#handlers.get.handle(incomingMessage, messageProtomux, connection, channelString);
        }
        else if (this.#isResponse(incomingMessage)) {
            await this.#handlers.response.handle(incomingMessage, connection, channelString);
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
