import ReadyResource from 'ready-resource';
import Hyperswarm from 'hyperswarm';
import w from 'protomux-wakeup';
import b4a from 'b4a';
import TransactionPoolService from './services/TransactionPoolService.js';
import ValidatorObserverService from './services/ValidatorObserverService.js';
import NetworkMessages from './protocols/NetworkMessages.js';
import { sleep } from '../../utils/helpers.js';
import {
    TRAC_NAMESPACE,
    MAX_PEERS,
    MAX_PARALLEL,
    MAX_SERVER_CONNECTIONS,
    MAX_CLIENT_CONNECTIONS,
    NETWORK_MESSAGE_TYPES
} from '../../utils/constants.js';
import ConnectionManager from './services/ConnectionManager.js';
import MessageOrchestrator from './services/MessageOrchestrator.js';
import NetworkWalletFactory from './identity/NetworkWalletFactory.js';
import { EventType } from '../../utils/constants.js';
import { networkMessageFactory } from '../../messages/network/v1/networkMessageFactory.js';
import TransactionRateLimiterService from './services/TransactionRateLimiterService.js';
// -- Debug Mode --
// TODO: Implement a better debug system in the future. This is just temporary.
const DEBUG = false;
const debugLog = (...args) => {
    if (DEBUG) {
        console.log('DEBUG [Network] ==> ', ...args);
    }
};

const wakeup = new w();

class Network extends ReadyResource {
    #swarm = null;
    #networkMessages;
    #transactionPoolService;
    #validatorObserverService;
    #validatorConnectionManager;
    #validatorMessageOrchestrator;
    #config;
    #identityProvider = null;
    #pendingConnections;
    #connectTimeoutMs;
    #maxPendingConnections;
    #rateLimiter;

    /**
     * @param {State} state
     * @param {object} config
     * @param {string} address
     **/
    constructor(state, config, address = null) {
        super();
        this.#config = config
        this.#connectTimeoutMs = config.connectTimeoutMs || 5000;
        this.#maxPendingConnections = config.maxPendingConnections || 50;
        this.#pendingConnections = new Map();
        this.#transactionPoolService = new TransactionPoolService(state, address, this.#config);
        this.#validatorObserverService = new ValidatorObserverService(this, state, address, this.#config);
        this.#validatorConnectionManager = new ConnectionManager(this.#config);
        this.#validatorMessageOrchestrator = new MessageOrchestrator(this.#validatorConnectionManager, state, this.#config);

    }

    get swarm() {
        return this.#swarm;
    }

    get transactionPoolService() {
        return this.#transactionPoolService;
    }

    get validatorObserverService() {
        return this.#validatorObserverService;
    }

    get validatorConnectionManager() {
        return this.#validatorConnectionManager;
    }

    get validatorMessageOrchestrator() {
        return this.#validatorMessageOrchestrator;
    }

    async _open() {
        console.log('Network initialization...');

        this.setupNetworkListeners();

        this.transactionPoolService.start();
        this.validatorObserverService.start();
    }

    async _close() {
        // TODO: Implement better "await" logic for stopping services
        console.log('Network: closing gracefully...');
        this.transactionPoolService.stopPool();
        await sleep(100);
        this.#validatorObserverService.stopValidatorObserver();
        await sleep(5_000);

        this.cleanupNetworkListeners();
        this.cleanupPendingConnections();

        if (this.#swarm !== null) {
            this.#swarm.destroy();
        }
    }

    setupNetworkListeners() {
        // VALIDATOR_CONNECTION_TIMEOUT
        this.on(EventType.VALIDATOR_CONNECTION_TIMEOUT, ({ publicKey, type, timeoutMs }) => {
            debugLog(`Network Event: VALIDATOR_CONNECTION_TIMEOUT | PublicKey: ${publicKey} | Type: ${type} | TimeoutMs: ${timeoutMs}`);
            this.#pendingConnections.delete(publicKey);
        });

        // VALIDATOR_CONNECTION_READY
        this.on(EventType.VALIDATOR_CONNECTION_READY, async ({ publicKey, type, connection }) => {
            debugLog(`Network Event: VALIDATOR_CONNECTION_READY | PublicKey: ${publicKey} | Type: ${type}`);
            const { timeoutId } = this.#pendingConnections.get(publicKey);
            if (!timeoutId) return;

            clearTimeout(timeoutId);
            this.#pendingConnections.delete(publicKey);

            if (type === 'validator') {
                await connection.protocolSession.send(NETWORK_MESSAGE_TYPES.GET.VALIDATOR);
            }

        });
    }

    cleanupNetworkListeners() {
        this.removeAllListeners(EventType.VALIDATOR_CONNECTION_TIMEOUT);
        this.removeAllListeners(EventType.VALIDATOR_CONNECTION_READY);
    }

    cleanupPendingConnections() {
        for (const { timeoutId } of this.#pendingConnections.values()) {
            clearTimeout(timeoutId);
        }
        this.#pendingConnections.clear();
    }

    async replicate(
        state,
        store,
        wallet,
    ) {
        if (!this.#swarm) {
            const keyPair = await this.initializeNetworkingKeyPair(store, wallet);
            const wrappedWallet = this.#getNetworkWalletWrapper(wallet, keyPair);
            this.#swarm = new Hyperswarm({
                keyPair,
                bootstrap: this.#config.dhtBootstrap,
                maxPeers: MAX_PEERS,
                maxParallel: MAX_PARALLEL,
                maxServerConnections: MAX_SERVER_CONNECTIONS,
                maxClientConnections: MAX_CLIENT_CONNECTIONS
            });

            this.#rateLimiter = new TransactionRateLimiterService(this.#swarm);
            this.#networkMessages = new NetworkMessages(
                state,
                wrappedWallet,
                this.#rateLimiter,
                this.#transactionPoolService,
                this.#validatorConnectionManager,
                this.#config
            );

            console.log(`Channel: ${b4a.toString(this.#config.channel)}`);

            this.#swarm.on('connection', async (connection) => {
                // Per-peer connection initialization:
                // - attach Protomux (legacy + v1 channels/messages)
                // - attach connection.protocolSession (used later by tryConnect / orchestrators to send messages)
                await this.#networkMessages.setupProtomuxMessages(connection);

                // ATTENTION: Must be called AFTER the protomux init above
                const stream = store.replicate(connection);
                wakeup.addStream(stream);

                const publicKey = b4a.toString(connection.remotePublicKey, 'hex');
                if (this.#pendingConnections.has(publicKey)) {
                    const { type } = this.#pendingConnections.get(publicKey);
                    await this.#finalizeConnection(publicKey, type, connection);
                }

                connection.on('close', () => {
                    this.#swarm.leavePeer(connection.remotePublicKey);
                    this.#validatorConnectionManager.remove(publicKey);
                    connection.protocolSession.close();
                });

                connection.on('error', (error) => {
                    if (
                        error && error.message && (
                            error.message.includes('connection reset by peer') ||
                            error.message.includes('Duplicate connection') ||
                            error.message.includes('connection timed out'))
                    ) {
                        // TODO: decide if we want to handle this error in a specific way. It generates a lot of logs.
                        return;
                    }
                    console.error(error.message)
                });

            });

            this.#swarm.join(this.#config.channel, { server: true, client: true });
            this.#swarm.flush();
        }
    }

    isConnectionPending(publicKey) {
        return this.#pendingConnections.has(publicKey);
    }

    pendingConnectionsCount() {
        return this.#pendingConnections.size;
    }

    async initializeNetworkingKeyPair(store, wallet) {
        if (!this.#config.enableWallet) {
            return await store.createKeyPair(TRAC_NAMESPACE);
        } else {
            return {
                publicKey: wallet.publicKey,
                secretKey: wallet.secretKey
            };
        }
    }

    async tryConnect(publicKey, type = null) {
        if (this.#swarm === null) throw new Error('Network swarm is not initialized');
        if (this.#pendingConnections.has(publicKey) || this.#pendingConnections.size >= this.#maxPendingConnections) {
            debugLog(`Network.tryConnect: Connection to peer: ${publicKey} as type: ${type} is already pending or max pending connections reached.`);
            return;
        }

        const timeoutId = setTimeout(() => {
            if (!this.#pendingConnections.has(publicKey)) return;
            this.emit(EventType.VALIDATOR_CONNECTION_TIMEOUT, { publicKey, type, timeoutMs: this.#connectTimeoutMs });
        }, this.#connectTimeoutMs);
        this.#pendingConnections.set(publicKey, { type, timeoutId });

        const target = b4a.from(publicKey, 'hex');
        if (!this.#swarm.peers.has(publicKey)) {
            this.#swarm.joinPeer(target);
        }

        const peerInfo = this.#swarm.peers.get(publicKey);
        if (peerInfo) {
            const connection = this.#swarm._allConnections.get(peerInfo.publicKey);
            if (connection && connection.protocolSession) {
                await this.#finalizeConnection(publicKey, type, connection);
            }
        }
    }

    async isConnected(publicKey) {
        return this.#swarm.peers.has(publicKey) &&
            this.#swarm.peers.get(publicKey).connectedTime != -1
    }

    async #finalizeConnection(publicKey, type, connection) {
        if (!this.#pendingConnections.has(publicKey)) return;
        this.emit(EventType.VALIDATOR_CONNECTION_READY, { publicKey, type, connection });
        debugLog(`Network.finalizeConnection: Connected to peer: ${publicKey} as type: ${type}`);
    }

    #getNetworkWalletWrapper(wallet, keyPair) {
        if (!this.#identityProvider) {
            this.#identityProvider = NetworkWalletFactory.provide({
                enableWallet: this.#config.enableWallet,
                wallet,
                keyPair,
                networkPrefix: this.#config.addressPrefix
            });
        }
        return this.#identityProvider;
    }

}

export default Network;
