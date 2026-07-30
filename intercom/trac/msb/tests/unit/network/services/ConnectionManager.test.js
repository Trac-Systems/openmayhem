import sinon from "sinon";
import { hook, test } from 'brittle'
import { default as EventEmitter } from "bare-events"
import { testKeyPair1, testKeyPair2, testKeyPair3, testKeyPair4, testKeyPair5, testKeyPair6, testKeyPair7, testKeyPair8 } from "../../../fixtures/apply.fixtures.js";
import ConnectionManager, { ConnectionManagerError } from "../../../../src/core/network/services/ConnectionManager.js";
import { tick } from "../../../helpers/setupApplyTests.js";
import b4a from 'b4a'
import { createConfig, ENV } from "../../../../src/config/env.js";
import { EventType, ResultCode } from "../../../../src/utils/constants.js";

const createConnection = (key) => {
    const emitter = new EventEmitter()
    emitter.protocolSession = {
        has: (name) => name === 'legacy',
        send: sinon.stub().resolves(),
    };
    emitter.connected = true
    emitter.remotePublicKey = b4a.from(key, 'hex')

    return { key: b4a.from(key, 'hex'), connection: emitter }
}

const createV1Connection = (key, sendHealthCheckStub = sinon.stub().resolves(ResultCode.OK)) => {
    const emitter = new EventEmitter()
    emitter.protocolSession = {
        sendHealthCheck: sendHealthCheckStub
    };
    emitter.connected = true
    emitter.remotePublicKey = b4a.from(key, 'hex')
    emitter.end = sinon.stub()

    return { key: b4a.from(key, 'hex'), connection: emitter }
}

const makeHealthCheckService = () => {
    const emitter = new EventEmitter();
    emitter.has = sinon.stub().returns(true);
    emitter.stop = sinon.stub();
    return emitter;
};

let connections

const makeManager = (maxValidators = 6, conns = null) => {
    const merged = createConfig(ENV.DEVELOPMENT, { maxValidators })
    const connectionManager = new ConnectionManager(merged)
    const activeConnections = conns ?? connections;

    activeConnections.forEach(({ key, connection }) => {
        connectionManager.addValidator(key, connection)
    });

    return connectionManager
}

const reset = () => {
    sinon.restore()
    connections.forEach(connection => {
        connection.connection.protocolSession.send.resetHistory()
    })
}
hook('Initialize state', async () => {
    connections = [
        createConnection(testKeyPair1.publicKey),
        createConnection(testKeyPair2.publicKey),
        createConnection(testKeyPair3.publicKey),
        createConnection(testKeyPair4.publicKey),
    ]
});

test('ConnectionManager', () => {
    test('addValidator', async () => {
        test('adds a validator', async t => {
            reset()
            const connectionManager = makeManager()
            t.is(connectionManager.connectionCount(), connections.length, 'should have the same length')
            const data = createConnection(testKeyPair5.publicKey)
            connectionManager.addValidator(data.key, data.connection)
            t.is(connectionManager.connectionCount(), connections.length + 1, 'should have the same length')
        })

        test('dont surpass maxConnections', async t => {
            reset()
            const maxConnections = 5
            const connectionManager = makeManager(maxConnections)
            t.is(connectionManager.connectionCount(), connections.length, 'should have the same length')

            const toAdd = createConnection(testKeyPair5.publicKey)
            connectionManager.addValidator(toAdd.key, toAdd.connection)
            t.is(connectionManager.connectionCount(), maxConnections, 'should match the max connections')

            const toNotAdd = createConnection(testKeyPair6.publicKey)
            connectionManager.addValidator(toNotAdd.key, toNotAdd.connection)
            t.is(connectionManager.connectionCount(), maxConnections, 'should not increase length')
        })

        test('does not add new validator when pool is full', async t => {
            reset()
            const maxConnections = 2
            const localConnections = [
                createConnection(testKeyPair1.publicKey),
                createConnection(testKeyPair2.publicKey),
            ]

            const connectionManager = makeManager(maxConnections)
            localConnections.forEach(({ key, connection }) => {
                connectionManager.addValidator(key, connection)
            })

            t.is(connectionManager.connectionCount(), maxConnections, 'pool should be full')

            const newConn = createConnection(testKeyPair3.publicKey)
            connectionManager.addValidator(newConn.key, newConn.connection)

            t.is(connectionManager.connectionCount(), maxConnections, 'should stay at max size')
            t.not(connectionManager.connected(newConn.key), 'new validator should not be in the pool')

            const remainingOld = localConnections.filter(c => connectionManager.connected(c.key)).length
            t.is(remainingOld, 2, 'all of the old validators should remain')
        })
    })

    test('connected', async () => {
        test('true', async t => {
            reset()
            const connectionManager = makeManager()
            connections.forEach(con => {
                t.ok(connectionManager.connected(con.key), 'should respond true')
            })
        })

        test('false', async t => {
            reset()
            const connectionManager = makeManager()
            t.ok(!connectionManager.connected(testKeyPair6.publicKey), 'should respond false')
        })
    })

    test('sendSingleMessage', async () => {
        test('returns exact resultCode from protocolSession.send', async t => {
            reset()
            const data = createConnection(testKeyPair1.publicKey)
            data.connection.protocolSession.send = sinon.stub().resolves(ResultCode.TIMEOUT)
            const connectionManager = makeManager(6, [data])

            const result = await connectionManager.sendSingleMessage({ payload: 1 }, testKeyPair1.publicKey)

            t.is(result, ResultCode.TIMEOUT, 'should return the exact result code from protocol session')
            t.ok(data.connection.protocolSession.send.calledOnce, 'should invoke protocolSession.send')
        })

        test('throws ConnectionManagerError when validator is disconnected', async t => {
            reset()
            const connectionManager = makeManager()

            try {
                await connectionManager.sendSingleMessage({ payload: 1 }, testKeyPair8.publicKey)
                t.fail('expected sendSingleMessage to throw')
            } catch (error) {
                t.ok(error instanceof ConnectionManagerError, 'should throw ConnectionManagerError')
                t.ok(error.message.includes('is not connected'), 'should include disconnected validator details')
            }
        })

        test('throws ConnectionManagerError when protocolSession is missing', async t => {
            reset()
            const emitter = new EventEmitter()
            emitter.connected = true
            emitter.remotePublicKey = b4a.from(testKeyPair6.publicKey, 'hex')
            emitter.end = sinon.stub()
            const data = {
                key: b4a.from(testKeyPair6.publicKey, 'hex'),
                connection: emitter,
            }

            const connectionManager = makeManager(6, [data])

            try {
                await connectionManager.sendSingleMessage({ payload: 1 }, testKeyPair6.publicKey)
                t.fail('expected sendSingleMessage to throw')
            } catch (error) {
                t.ok(error instanceof ConnectionManagerError, 'should throw ConnectionManagerError')
                t.ok(error.message.includes('no valid connection found'), 'should include protocol session details')
            }
        })
    })

    // Note: These tests were commented out because connectionManager.send is being deprecated. When it is completely removed, the tests should be deleted.
    // test('send', async t => {
    //     // test('triggers send on messenger', async t => {
    //     //     reset()
    //     //     const connectionManager = makeManager()

    //     //     const target = connectionManager.send([1,2,3,4])

    //     //     const totalCalls = connections.reduce((sum, con) => sum + con.connection.protocolSession.send.callCount, 0)
    //     //     t.is(totalCalls, 1, 'should send to exactly one validator')
    //     //     t.ok(target, 'should return a target public key')
    //     // })

    //     test('does not throw on individual send errors', async t => {
    //         reset()
    //         const errorConnections = [
    //             createConnection(testKeyPair7.publicKey),
    //             createConnection(testKeyPair8.publicKey),
    //         ]

    //         errorConnections.forEach(con => {
    //             con.connection.protocolSession.send = sinon.stub().throws(new Error())
    //         })

    //         const connectionManager = makeManager(5, errorConnections)

    //         t.is(errorConnections.length, 2, 'should have two connections')
    //         connectionManager.send([1,2,3,4])
    //         t.ok(true, 'send should not throw even if individual sends fail')
    //     })
    // })

    test('on close', async () => {
        test('removes from list', async t => {
            reset()
            const connectionManager = makeManager()

            const connectionCount = connectionManager.connectionCount()

            connections[1].connection.connected = false
            connections[1].connection.emit('close')
            await tick()
            t.is(connectionCount, connectionManager.connectionCount() + 1, 'first on the list should have been called')
        })
    })

    test('remove', async () => {
        test('removes a validator by public key', async t => {
            reset()
            const connectionManager = makeManager()
            const previousCount = connectionManager.connectionCount()
            const lastValidator = connections.shift()

            t.ok(connectionManager.connected(lastValidator.key), 'should be connected')
            connectionManager.remove(lastValidator.key)

            t.is(connectionManager.connectionCount(), previousCount - 1, 'should reduce the connection count')
            t.ok(!connectionManager.connected(lastValidator.key), 'should be connected')
        })

        test('can detach a validator without ending the socket', async t => {
            reset()
            const data = createV1Connection(testKeyPair5.publicKey)
            const connectionManager = makeManager(6, [data])

            connectionManager.remove(data.key, { endConnection: false })

            t.absent(connectionManager.connected(data.key), 'validator should be removed from the pool')
            t.is(data.connection.end.callCount, 0, 'socket should remain open for in-flight responses')
        })
    })

    test('on close', async () => {
        test('removes from list', async t => {
            reset()
            const connectionManager = makeManager()

            const connectionCount = connectionManager.connectionCount()

            connections[1].connection.connected = false
            connections[1].connection.emit('close')
            await tick()
            t.is(connectionCount, connectionManager.connectionCount() + 1, 'first on the list should have been called')
        })
    })

    test('health checks (strict)', async () => {
        test('keeps validator on OK response', async t => {
            try {
                const v1Conn = createV1Connection(testKeyPair1.publicKey, sinon.stub().resolves(ResultCode.OK));
                const connectionManager = makeManager(6, [v1Conn]);
                const healthCheckService = makeHealthCheckService();
                connectionManager.subscribeToHealthChecks(healthCheckService);

                healthCheckService.emit(
                    EventType.VALIDATOR_HEALTH_CHECK,
                    testKeyPair1.publicKey,
                    "123456"
                );

                await tick();
                t.ok(connectionManager.connected(v1Conn.key));
                t.is(healthCheckService.stop.callCount, 0);
            } finally {
                sinon.restore();
            }
        });

        test('removes validator on non-OK response', async t => {
            try {
                const v1Conn = createV1Connection(testKeyPair2.publicKey, sinon.stub().resolves(ResultCode.TIMEOUT));
                const connectionManager = makeManager(6, [v1Conn]);
                const healthCheckService = makeHealthCheckService();
                connectionManager.subscribeToHealthChecks(healthCheckService);

                healthCheckService.emit(
                    EventType.VALIDATOR_HEALTH_CHECK,
                    testKeyPair2.publicKey,
                    "123456"
                );

                await tick();
                t.ok(!connectionManager.connected(v1Conn.key));
                t.ok(healthCheckService.stop.callCount >= 1);
            } finally {
                sinon.restore();
            }
        });

        test('removes validator on send rejection', async t => {
            try {
                const v1Conn = createV1Connection(testKeyPair3.publicKey, sinon.stub().rejects(new Error('boom')));
                const connectionManager = makeManager(6, [v1Conn]);
                const healthCheckService = makeHealthCheckService();
                connectionManager.subscribeToHealthChecks(healthCheckService);

                healthCheckService.emit(
                    EventType.VALIDATOR_HEALTH_CHECK,
                    testKeyPair3.publicKey,
                    "123456"
                );

                await tick();
                t.ok(!connectionManager.connected(v1Conn.key));
                t.ok(healthCheckService.stop.callCount >= 1);
            } finally {
                sinon.restore();
            }
        });

        test('ignores malformed health check events', async t => {
            try {
                const v1Conn = createV1Connection(testKeyPair5.publicKey, sinon.stub().resolves(ResultCode.OK));
                const connectionManager = makeManager(6, [v1Conn]);
                let handler = null;
                const healthCheckService = {
                    on: (_event, fn) => { handler = fn; },
                    off: () => {},
                    has: sinon.stub().returns(true),
                    stop: sinon.stub()
                };
                connectionManager.subscribeToHealthChecks(healthCheckService);

                const cases = [
                    { label: 'publicKey', publicKey: 123, requestId: 'abc' },
                    { label: 'requestId', publicKey: testKeyPair5.publicKey, requestId: 456 },
                    { label: 'undefined', publicKey: undefined, requestId: undefined },
                ];

                for (const testCase of cases) {
                    await handler(testCase.publicKey, testCase.requestId);
                    t.pass(`ignored malformed payload: ${testCase.label}`);
                }
            } finally {
                sinon.restore();
            }
        });
    })

    test('edge branches', async () => {
        test('pickRandomValidator returns null for empty array', async t => {
            reset()
            const connectionManager = makeManager()
            t.is(connectionManager.pickRandomValidator([]), null)
        })

        test('pickRandomConnectedValidator returns null when pool is empty', async t => {
            reset()
            const connectionManager = makeManager(6, [])
            t.is(connectionManager.pickRandomConnectedValidator(), null)
        })

        test('remove missing validator keeps state unchanged', async t => {
            reset()
            const connectionManager = makeManager()
            const before = connectionManager.connectionCount()
            connectionManager.remove(testKeyPair8.publicKey)
            t.is(connectionManager.connectionCount(), before)
        })

        test('remove handles connection.end throwing and still deletes validator', async t => {
            reset()
            const data = createConnection(testKeyPair7.publicKey)
            data.connection.end = sinon.stub().throws(new Error('end boom'))
            const connectionManager = makeManager(6, [data])

            t.ok(connectionManager.connected(data.key))
            connectionManager.remove(data.key)
            t.absent(connectionManager.connected(data.key))
        })

        test('sent counters handle missing validators safely', async t => {
            reset()
            const connectionManager = makeManager()
            t.is(connectionManager.getSentCount(testKeyPair8.publicKey), 0)
            connectionManager.incrementSentCount(testKeyPair8.publicKey)
            t.is(connectionManager.getSentCount(testKeyPair8.publicKey), 0)
        })

        test('subscribeToHealthChecks validates service interface', async t => {
            reset()
            const connectionManager = makeManager()

            await t.exception(
                () => connectionManager.subscribeToHealthChecks({ on() {} }),
                /must implement on\/off/
            )
        })

        test('health check removes validator when protocolSession is missing', async t => {
            reset()
            const emitter = new EventEmitter()
            emitter.connected = true
            emitter.remotePublicKey = b4a.from(testKeyPair6.publicKey, 'hex')
            emitter.end = sinon.stub()
            const data = {
                key: b4a.from(testKeyPair6.publicKey, 'hex'),
                connection: emitter
            }

            const connectionManager = makeManager(6, [data])
            const healthCheckService = {
                on: (_event, fn) => { healthCheckService.handler = fn; },
                off: () => {},
                has: sinon.stub().returns(true),
                stop: sinon.stub(),
                handler: null,
            }

            connectionManager.subscribeToHealthChecks(healthCheckService)
            await healthCheckService.handler(testKeyPair6.publicKey, 'hc-1')

            t.absent(connectionManager.connected(data.key))
            t.ok(healthCheckService.stop.called)
        })

        test('remove tolerates health check service errors', async t => {
            reset()
            const data = createConnection(testKeyPair5.publicKey)
            const connectionManager = makeManager(6, [data])
            const healthCheckService = {
                on: (_event, fn) => { healthCheckService.handler = fn; },
                off: () => {},
                has: sinon.stub().throws(new Error('has boom')),
                stop: sinon.stub(),
                handler: null,
            }
            connectionManager.subscribeToHealthChecks(healthCheckService)

            connectionManager.remove(data.key)

            t.absent(connectionManager.connected(data.key))
        })
    })
})
