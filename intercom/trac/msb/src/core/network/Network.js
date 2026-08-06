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
    EventType,
    CONNECTION_STATUS
} from '../../utils/constants.js';
import ConnectionManager from './services/ConnectionManager.js';
import MessageOrchestrator from './services/MessageOrchestrator.js';
import TransactionRateLimiterService from './services/TransactionRateLimiterService.js';
import PendingRequestService from './services/PendingRequestService.js';
import TransactionCommitService from "./services/TransactionCommitService.js";
import ValidatorHealthCheckService from './services/ValidatorHealthCheckService.js';
import { Logger } from '../../utils/logger.js';
import { WalletProvider } from 'trac-wallet';

const wakeup = new w();

class Network extends ReadyResource {
    #swarm = null;
    #networkMessages;
    #transactionPoolService;
    #validatorObserverService;
    #validatorConnectionManager;
    #validatorMessageOrchestrator;
    #config;
    #pendingConnections;
    #connectTimeoutMs;
    #maxPendingConnections;
    #rateLimiter;
    #pendingRequestsService;
    #transactionCommitService;
    #wallet;
    #validatorHealthCheckService;
    #logger;
    #closing = false;

    /**
     * @param {State} state
     * @param {Config} config
     * @param {string} address
     **/
    constructor(state, config, address = null) {
        super();
        this.#config = config
        this.#connectTimeoutMs = config.connectTimeoutMs || 5000;
        this.#maxPendingConnections = config.maxPendingConnections || 50;
        this.#pendingConnections = new Map();
        this.#transactionCommitService = new TransactionCommitService(this.#config);
        this.#transactionPoolService = new TransactionPoolService(state, address, this.#transactionCommitService ,this.#config);
        this.#validatorObserverService = new ValidatorObserverService(this, state, address, this.#config);
        this.#validatorConnectionManager = new ConnectionManager(this.#config);
        this.#validatorMessageOrchestrator = new MessageOrchestrator(this.#validatorConnectionManager, state, this.#config);
        this.#pendingRequestsService = new PendingRequestService(this.#config);
        this.#logger = new Logger(this.#config);
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
        this.#logger.info('Network initialization...');
        this.#closing = false;

        this.setupNetworkListeners();

        this.transactionPoolService.start();
        this.validatorObserverService.start();
    }

    async _close() {
        this.#logger.info('Network: closing gracefully...');
        this.#closing = true;
        await this.transactionPoolService.stopPool();
        await sleep(100);
        await this.#validatorObserverService.stopValidatorObserver();
        await sleep(5_000);
        if (this.#validatorHealthCheckService) {
            await this.#validatorHealthCheckService.close();
        }

        this.cleanupNetworkListeners();
        this.cleanupPendingConnections();
        this.#pendingRequestsService.close();
        this.#transactionCommitService.close();

        const swarm = this.#swarm;
        if (swarm !== null) {
            if (typeof swarm.removeAllListeners === 'function') {
                swarm.removeAllListeners('connection');
            }
            await swarm.destroy();
            if (this.#swarm === swarm) {
                this.#swarm = null;
            }
        }
    }

    setupNetworkListeners() {
        this.on(EventType.VALIDATOR_CONNECTION_TIMEOUT, ({ publicKey, type, timeoutMs }) => {
            this.#logger.debug(`Network Event: VALIDATOR_CONNECTION_TIMEOUT | PublicKey: ${publicKey} | Type: ${type} | TimeoutMs: ${timeoutMs}`);
            this.#pendingConnections.delete(publicKey);
        });

        this.on(EventType.VALIDATOR_CONNECTION_READY, async ({ publicKey, type, connection }) => {
            this.#logger.debug(`Network Event: VALIDATOR_CONNECTION_READY | PublicKey: ${publicKey} | Type: ${type}`);
            const { timeoutId } = this.#pendingConnections.get(publicKey);

            if (!timeoutId) return;

            clearTimeout(timeoutId);
            this.#pendingConnections.delete(publicKey);

            if (type === 'validator') {
                try {
                    if (!connection.protocolSession.isProbed()) await connection.protocolSession.probe();
                } catch (err) {
                    this.#logger.debug(`failed to probe peer with publicKey ${publicKey}: ${err?.message ?? err}`);
                }

                this.#validatorConnectionManager.addValidator(publicKey, connection);

                let healthCheckSupported = false;
                try {
                    healthCheckSupported = connection.protocolSession.isHealthCheckSupported();
                } catch (err) {
                    this.#logger.debug(`health check support unknown for peer with publicKey ${publicKey}: ${err?.message ?? err}`);
                }

                if (healthCheckSupported) {
                    this.#validatorHealthCheckService.start(publicKey);
                } else {
                    this.#validatorHealthCheckService.stop(publicKey);
                }
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
            const { wallet: wrappedWallet, keyPair } = await this.#getOrGenerateWallet(store, wallet);
            this.#wallet = wrappedWallet
            this.#validatorMessageOrchestrator.setWallet(this.#wallet);

            this.#swarm = new Hyperswarm({
                keyPair,
                bootstrap: this.#config.dhtBootstrap,
                maxPeers: this.#config.maxPeers,
                maxParallel: this.#config.maxParallel,
                maxServerConnections: this.#config.maxServerConnections,
                maxClientConnections: this.#config.maxClientConnections
            });

            this.#rateLimiter = new TransactionRateLimiterService(this.#swarm, this.#config);
            this.#networkMessages = new NetworkMessages(
                state,
                this.#wallet,
                this.#rateLimiter,
                this.#transactionPoolService,
                this.#pendingRequestsService,
                this.#transactionCommitService,
                this.#config
            );
            this.#validatorHealthCheckService = new ValidatorHealthCheckService(this.#config);
            await this.#validatorHealthCheckService.ready();
            this.#validatorConnectionManager.subscribeToHealthChecks(this.#validatorHealthCheckService);

            this.#logger.info(`Channel: ${b4a.toString(this.#config.channel)}`);

            this.#swarm.on('connection', async (connection) => {
                if (this.#closing) {
                    this.#destroyConnection(connection);
                    return;
                }

                try {
                    // Per-peer connection initialization:
                    // - attach Protomux (legacy + v1 channels/messages)
                    // - attach connection.protocolSession (used later by tryConnect / orchestrators to send messages)
                    await this.#networkMessages.setupProtomuxMessages(connection);

                    // Pear v3 can deliver a late swarm connection while shutdown is
                    // already closing the Corestore. Do not replicate a connection
                    // after close has begun; store.replicate would otherwise throw.
                    if (this.#closing || this.#swarm === null) {
                        this.#destroyConnection(connection);
                        return;
                    }

                    // ATTENTION: Must be called AFTER the protomux init above
                    const stream = store.replicate(connection);
                    wakeup.addStream(stream);
                } catch (error) {
                    const publicKey = connection.remotePublicKey
                        ? b4a.toString(connection.remotePublicKey, 'hex')
                        : 'unknown';
                    this.#pendingRequestsService.rejectPendingRequestsForPeer(
                        publicKey,
                        error ?? new Error('Connection setup failed')
                    );
                    this.#destroyConnection(connection);
                    if (!this.#closing) {
                        this.#logger.error(error?.message ?? 'Unknown network connection setup error');
                    }
                    return;
                }

                const publicKey = b4a.toString(connection.remotePublicKey, 'hex');
                if (this.#pendingConnections.has(publicKey)) {
                    const { type } = this.#pendingConnections.get(publicKey);
                    await this.#finalizeConnection(publicKey, type, connection);
                }

                connection.on('close', () => {
                    this.#pendingRequestsService.rejectPendingRequestsForPeer(
                        publicKey,
                        new Error('Connection closed before response')
                    );
                    this.#swarm?.leavePeer(connection.remotePublicKey);
                    this.#validatorConnectionManager.remove(publicKey);
                    if (connection.protocolSession) {
                        try {
                            connection.protocolSession.close();
                        } catch {}
                    }
                });

                connection.on('error', (error) => {
                    this.#pendingRequestsService.rejectPendingRequestsForPeer(
                        publicKey,
                        error ?? new Error('Connection error before response')
                    );
                    if (
                        error && error.message && (
                            error.message.includes('connection reset by peer') ||
                            error.message.includes('Duplicate connection') ||
                            error.message.includes('connection timed out'))
                    ) {
                        // TODO: decide if we want to handle this error in a specific way. It generates a lot of logs.
                        return;
                    }
                    this.#logger.error(error?.message ?? 'Unknown network connection error');
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

    disconnectValidatorPeer(publicKey, reason = 'validator peer invalidated') {
        const publicKeyHex = this.#normalizePublicKey(publicKey);
        if (!publicKeyHex) return false;

        const hadPendingValidatorConnection = this.#clearPendingValidatorConnection(publicKeyHex);
        const isTrackedValidator = this.#validatorConnectionManager.exists(publicKeyHex);

        const shouldLeavePeer = hadPendingValidatorConnection || isTrackedValidator;

        if (shouldLeavePeer && this.#swarm?.peers?.has(publicKeyHex)) {
            this.#logger.debug(`Network.disconnectValidatorPeer: leaving peer ${publicKeyHex}. Reason: ${reason}`);
            this.#swarm.leavePeer(b4a.from(publicKeyHex, 'hex'));
        }

        if (isTrackedValidator) {
            this.#logger.debug(`Network.disconnectValidatorPeer: detaching tracked validator ${publicKeyHex}. Reason: ${reason}`);
            this.#validatorConnectionManager.remove(publicKeyHex, { endConnection: false });
        }

        return hadPendingValidatorConnection || isTrackedValidator;
    }

    async #getOrGenerateWallet(store, wallet) {
        if (!this.#config.enableWallet) {
            const keyPair = await store.createKeyPair(TRAC_NAMESPACE);
            const wallet = await new WalletProvider(this.#config).fromSecretKey(keyPair.secretKey)
            return { keyPair, wallet }
        } else {
            const keyPair = { publicKey: wallet.publicKey, secretKey: wallet.secretKey }
            return { keyPair, wallet }
        }
    }

    async tryConnect(publicKey, type = null) {
        if (this.#swarm === null) throw new Error('Network swarm is not initialized');
        if (this.#pendingConnections.has(publicKey) || this.#pendingConnections.size >= this.#maxPendingConnections) {
            this.#logger.debug(`Network.tryConnect: Connection to peer: ${publicKey} as type: ${type} is already pending or max pending connections reached.`);
            return CONNECTION_STATUS.IGNORED;
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

            if (connection &&
                connection.protocolSession &&
                !this.#pendingRequestsService.isProbePending(connection.remotePublicKey.toString('hex'))
            ) {
                await this.#finalizeConnection(publicKey, type, connection);
                return CONNECTION_STATUS.CONNECTED;
            }
        }
        
        return CONNECTION_STATUS.PENDING;
    }

    async #finalizeConnection(publicKey, type, connection) {
        if (!this.#pendingConnections.has(publicKey)) return;
        this.emit(EventType.VALIDATOR_CONNECTION_READY, { publicKey, type, connection });
        this.#logger.debug(`Network.finalizeConnection: Connected to peer: ${publicKey} as type: ${type}`);
    }

    #normalizePublicKey(publicKey) {
        if (typeof publicKey === 'string') return publicKey;
        if (b4a.isBuffer(publicKey)) return b4a.toString(publicKey, 'hex');
        return null;
    }

    #clearPendingValidatorConnection(publicKeyHex) {
        if (!this.#pendingConnections.has(publicKeyHex)) return false;

        const { timeoutId, type } = this.#pendingConnections.get(publicKeyHex);
        if (type !== 'validator') return false;

        clearTimeout(timeoutId);
        this.#pendingConnections.delete(publicKeyHex);
        return true;
    }

    #destroyConnection(connection) {
        if (!connection) return;
        if (connection.protocolSession) {
            try {
                connection.protocolSession.close();
            } catch {}
        }
        if (typeof connection.destroy === 'function') {
            connection.destroy();
        } else if (typeof connection.end === 'function') {
            connection.end();
        }
    }
}

export default Network;
