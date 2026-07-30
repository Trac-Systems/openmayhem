import sinon from 'sinon';
import { test } from 'brittle';
import b4a from 'b4a';

import TransactionRateLimiterService from '../../../../src/core/network/services/TransactionRateLimiterService.js';
import { V1ProtocolError } from '../../../../src/core/network/protocols/v1/V1ProtocolError.js';
import { ResultCode } from '../../../../src/utils/constants.js';
import { config } from '../../../helpers/config.js';
import { testKeyPair1, testKeyPair2 } from '../../../fixtures/apply.fixtures.js';

const CLEANUP_INTERVAL_MS = config.rateLimitCleanupIntervalMs;
const CONNECTION_TIMEOUT_MS = config.rateLimitConnectionTimeoutMs;
const MAX_TRANSACTIONS_PER_SECOND = config.rateLimitMaxTransactionsPerSecond;

const makeConnection = (publicKeyHex) => {
    return {
        remotePublicKey: b4a.from(publicKeyHex, 'hex'),
        end: sinon.stub()
    };
};

const makeSwarm = () => {
    return {
        leavePeer: sinon.stub()
    };
};

test('TransactionRateLimiterService', async () => {
    test('legacyHandleRateLimit disconnects after MAX+1 tx in the same second', async (t) => {
        const clock = sinon.useFakeTimers({ now: 0 });
        try {
            const swarm = makeSwarm();
            const limiter = new TransactionRateLimiterService(swarm, config);
            const connection = makeConnection(testKeyPair1.publicKey);

            for (let i = 0; i < MAX_TRANSACTIONS_PER_SECOND; i++) {
                t.is(limiter.legacyHandleRateLimit(connection), false);
            }

            t.is(limiter.legacyHandleRateLimit(connection), true);
            t.is(swarm.leavePeer.callCount, 1);
            t.alike(swarm.leavePeer.firstCall.args[0], connection.remotePublicKey);
            t.is(connection.end.callCount, 1);
        } finally {
            clock.restore();
            sinon.restore();
        }
    });

    test('legacyHandleRateLimit resets the window on the next second', async (t) => {
        const clock = sinon.useFakeTimers({ now: 0 });
        try {
            const swarm = makeSwarm();
            const limiter = new TransactionRateLimiterService(swarm, config);
            const connection = makeConnection(testKeyPair1.publicKey);

            for (let i = 0; i < MAX_TRANSACTIONS_PER_SECOND; i++) {
                t.is(limiter.legacyHandleRateLimit(connection), false);
            }

            clock.tick(1000);
            t.is(limiter.legacyHandleRateLimit(connection), false);
            t.is(swarm.leavePeer.callCount, 0);
            t.is(connection.end.callCount, 0);
        } finally {
            clock.restore();
            sinon.restore();
        }
    });

    test('v1HandleRateLimit throws V1ProtocolError after MAX+1 tx in the same second', async (t) => {
        const clock = sinon.useFakeTimers({ now: 0 });
        try {
            const limiter = new TransactionRateLimiterService(makeSwarm(), config);
            const connection = makeConnection(testKeyPair2.publicKey);

            for (let i = 0; i < MAX_TRANSACTIONS_PER_SECOND; i++) {
                limiter.v1HandleRateLimit(connection);
            }

            let err;
            try {
                limiter.v1HandleRateLimit(connection);
            } catch (error) {
                err = error;
            }

            t.ok(err instanceof V1ProtocolError);
            t.is(err.resultCode, ResultCode.RATE_LIMITED);
            t.ok(err.message.includes('Rate limit exceeded for peer'));
        } finally {
            clock.restore();
            sinon.restore();
        }
    });

    test('v1HandleRateLimit resets the window on the next second', async () => {
        const clock = sinon.useFakeTimers({ now: 0 });
        try {
            const limiter = new TransactionRateLimiterService(makeSwarm(), config);
            const connection = makeConnection(testKeyPair2.publicKey);

            for (let i = 0; i < MAX_TRANSACTIONS_PER_SECOND; i++) {
                limiter.v1HandleRateLimit(connection);
            }

            clock.tick(1000);
            limiter.v1HandleRateLimit(connection);
        } finally {
            clock.restore();
            sinon.restore();
        }
    });

    test('cleanUpOldConnections evicts inactive stats after cleanup interval', async (t) => {
        const clock = sinon.useFakeTimers({ now: 0 });
        try {
            const swarm = makeSwarm();
            const limiter = new TransactionRateLimiterService(swarm, config);

            const oldPeer = makeConnection(testKeyPair1.publicKey);
            const activePeer = makeConnection(testKeyPair2.publicKey);

            t.is(limiter.legacyHandleRateLimit(oldPeer), false);
            t.is(limiter.legacyHandleRateLimit(activePeer), false);

            clock.tick(CONNECTION_TIMEOUT_MS + 1);
            t.is(limiter.legacyHandleRateLimit(activePeer), false);

            clock.tick(CLEANUP_INTERVAL_MS - (CONNECTION_TIMEOUT_MS + 1));
            t.is(Date.now(), CLEANUP_INTERVAL_MS);
            t.is(limiter.legacyHandleRateLimit(activePeer), false);

            t.is(limiter.legacyHandleRateLimit(oldPeer), false);
            t.is(swarm.leavePeer.callCount, 0);
        } finally {
            clock.restore();
            sinon.restore();
        }
    });
});
