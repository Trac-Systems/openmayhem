
import b4a from 'b4a';
import NetworkMessageRouter from './legacy/NetworkMessageRouter.js';
import NetworkMessageRouterV1 from './v1/NetworkMessageRouter.js';
import ProtocolSession from './ProtocolSession.js';
import LegacyProtocol from './LegacyProtocol.js';
import V1Protocol from './V1Protocol.js';
class NetworkMessages {
    #legacyMessageRouter;
    #v1MessageRouter;
    #config;

    /**
     * @param {object} config
     **/
    constructor(state, wallet, rateLimiterService, txPoolService, connectionManager, config) {
        this.#config = config;
        this.#initializeMessageRouter(state, wallet, rateLimiterService, txPoolService, connectionManager);
    }

    #initializeMessageRouter(state, wallet, rateLimiterService, txPoolService, connectionManager) {
        this.#legacyMessageRouter = new NetworkMessageRouter(
            state,
            wallet,
            rateLimiterService,
            txPoolService,
            connectionManager,
            this.#config
        );
        this.#v1MessageRouter = new NetworkMessageRouterV1(this.#config);
    }

    async setupProtomuxMessages(connection) {
        // Attach a Protomux instance to this Hyperswarm connection.
        // Protomux multiplexes multiple logical protocol channels over a single encrypted stream.

        const legacyProtocol = new LegacyProtocol(this.#legacyMessageRouter, connection, this.#config);
        const v1Protocol = new V1Protocol(this.#v1MessageRouter, connection, this.#config);

        // ProtocolSession is attached to the Hyperswarm connection so other parts of the system (e.g. tryConnect)
        // can send messages without knowing how Protomux was initialized.
        const protocolSession = new ProtocolSession(legacyProtocol, v1Protocol);
        connection.protocolSession = protocolSession;

    }
}

export default NetworkMessages;
