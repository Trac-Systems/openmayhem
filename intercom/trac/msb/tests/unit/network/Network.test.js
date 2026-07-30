import { test } from 'brittle';
import sinon from 'sinon';
import b4a from 'b4a';
import EventEmitter from 'bare-events';
import { CONNECTION_STATUS } from '../../../src/utils/constants.js';

const isBareRuntime = typeof globalThis.Bare !== 'undefined';

function normalizePublicKey(publicKey) {
    if (typeof publicKey === 'string') return publicKey;
    if (b4a.isBuffer(publicKey)) return b4a.toString(publicKey, 'hex');
    return null;
}

async function loadNetwork() {
    const { default: esmock } = await import('esmock');
    let swarmInstance = null;
    let connectionManagerInstance = null;

    class HyperswarmMock extends EventEmitter {
        constructor() {
            super();
            swarmInstance = this;
            this.peers = new Map();
            this._allConnections = new Map();
            this.joinPeer = sinon.stub().callsFake((target) => {
                const publicKeyHex = b4a.toString(target, 'hex');
                this.peers.set(publicKeyHex, { publicKey: target });
            });
            this.leavePeer = sinon.stub();
            this.join = sinon.stub();
            this.flush = sinon.stub();
            this.destroy = sinon.stub();
        }
    }

    class ConnectionManagerMock {
        constructor() {
            connectionManagerInstance = this;
            this.validators = new Set();
            this.removed = [];
        }

        exists(publicKey) {
            return this.validators.has(normalizePublicKey(publicKey));
        }

        remove(publicKey, options = {}) {
            const publicKeyHex = normalizePublicKey(publicKey);
            this.removed.push({ publicKey: publicKeyHex, options });
            this.validators.delete(publicKeyHex);
        }

        addValidator(publicKey) {
            this.validators.add(normalizePublicKey(publicKey));
            return true;
        }

        connected(publicKey) {
            return this.exists(publicKey);
        }

        connectedValidators() {
            return Array.from(this.validators);
        }

        connectionCount() {
            return this.validators.size;
        }

        maxConnectionsReached() {
            return false;
        }

        subscribeToHealthChecks() {}
    }

    class TransactionPoolServiceMock {
        start() {}
        async stopPool() {}
    }

    class ValidatorObserverServiceMock {
        start() {}
        async stopValidatorObserver() {}
    }

    class MessageOrchestratorMock {
        setWallet() {}
    }

    class PendingRequestServiceMock {
        isProbePending() { return false; }
        rejectPendingRequestsForPeer() {}
        close() {}
    }

    class TransactionCommitServiceMock {
        close() {}
    }

    class ValidatorHealthCheckServiceMock extends EventEmitter {
        async ready() {}
        start() {}
        stop() {}
        has() { return false; }
        close() {}
    }

    class LoggerMock {
        info() {}
        debug() {}
        error() {}
    }

    class NetworkMessagesMock {
        async setupProtomuxMessages() {}
    }

    class TransactionRateLimiterServiceMock {}

    const NetworkModule = await esmock('../../../src/core/network/Network.js', {
        hyperswarm: HyperswarmMock,
        '../../../src/core/network/services/TransactionPoolService.js': { default: TransactionPoolServiceMock },
        '../../../src/core/network/services/ValidatorObserverService.js': { default: ValidatorObserverServiceMock },
        '../../../src/core/network/services/ConnectionManager.js': { default: ConnectionManagerMock },
        '../../../src/core/network/services/MessageOrchestrator.js': { default: MessageOrchestratorMock },
        '../../../src/core/network/services/TransactionRateLimiterService.js': { default: TransactionRateLimiterServiceMock },
        '../../../src/core/network/services/PendingRequestService.js': { default: PendingRequestServiceMock },
        '../../../src/core/network/services/TransactionCommitService.js': { default: TransactionCommitServiceMock },
        '../../../src/core/network/services/ValidatorHealthCheckService.js': { default: ValidatorHealthCheckServiceMock },
        '../../../src/core/network/protocols/NetworkMessages.js': { default: NetworkMessagesMock },
        '../../../src/utils/logger.js': { Logger: LoggerMock },
    });

    const Network = NetworkModule.default;
    const config = {
        enableWallet: true,
        connectTimeoutMs: 1_000,
        maxPendingConnections: 10,
        maxValidators: 5,
        maxPeers: 5,
        maxParallel: 1,
        maxServerConnections: 5,
        maxClientConnections: 5,
        dhtBootstrap: [],
        channel: b4a.alloc(32, 1),
    };

    const wallet = {
        publicKey: b4a.alloc(32, 2),
        secretKey: b4a.alloc(64, 3),
        address: 'trac_test',
    };

    const network = new Network({}, config, wallet.address);
    await network.replicate({}, {}, wallet);

    return { network, swarmInstance, connectionManagerInstance };
}

if (isBareRuntime) {
    test('Network#disconnectValidatorPeer coverage is Node-only', t => {
        t.pass('skipped in Bare because esmock depends on node:module');
    });
} else {
    test('Network#disconnectValidatorPeer clears pending validator attempts', async t => {
        const publicKey = 'a'.repeat(64);
        const { network, swarmInstance } = await loadNetwork();

        const status = await network.tryConnect(publicKey, 'validator');
        t.is(status, CONNECTION_STATUS.PENDING, 'connection attempt should remain pending');
        t.ok(network.isConnectionPending(publicKey), 'pending connection should be tracked before invalidation');

        const disconnected = network.disconnectValidatorPeer(publicKey, 'peer invalidated by state event');

        t.ok(disconnected, 'disconnect should report work done');
        t.absent(network.isConnectionPending(publicKey), 'pending connection should be cleared');
        t.is(swarmInstance.leavePeer.callCount, 1, 'peer discovery should be cancelled');
    });

    test('Network#disconnectValidatorPeer removes tracked validators from the pool', async t => {
        const publicKey = 'b'.repeat(64);
        const { network, swarmInstance, connectionManagerInstance } = await loadNetwork();

        connectionManagerInstance.addValidator(publicKey);
        swarmInstance.peers.set(publicKey, { publicKey: b4a.from(publicKey, 'hex') });

        const disconnected = network.disconnectValidatorPeer(publicKey, 'peer no longer valid validator');

        t.ok(disconnected, 'disconnect should report tracked validator removal');
        t.absent(connectionManagerInstance.exists(publicKey), 'validator should be removed from connection manager');
        t.alike(connectionManagerInstance.removed, [{ publicKey, options: { endConnection: false } }], 'tracked validator should be detached without ending the socket');
        t.is(swarmInstance.leavePeer.callCount, 1, 'leavePeer should be called to clear explicit peer tracking without closing the socket');
    });

    test('Network#disconnectValidatorPeer ignores non-validator pending peers', async t => {
        const publicKey = 'c'.repeat(64);
        const { network, swarmInstance } = await loadNetwork();

        const status = await network.tryConnect(publicKey, 'rpc');
        t.is(status, CONNECTION_STATUS.PENDING, 'non-validator connection attempt should be pending');
        t.ok(network.isConnectionPending(publicKey), 'non-validator pending connection should be tracked');

        const disconnected = network.disconnectValidatorPeer(publicKey, 'state event should not affect generic peer');

        t.absent(disconnected, 'non-validator peer should be ignored by validator disconnect helper');
        t.ok(network.isConnectionPending(publicKey), 'non-validator pending connection should remain tracked');
        t.is(swarmInstance.leavePeer.callCount, 0, 'generic peer should not be left');
    });
}
