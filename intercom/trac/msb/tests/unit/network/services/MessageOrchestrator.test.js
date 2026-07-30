import { hook, test } from 'brittle';
import sinon from 'sinon';
import MessageOrchestrator from '../../../../src/core/network/services/MessageOrchestrator.js';
import State from '../../../../src/core/state/State.js';
import { OperationType, ResultCode } from '../../../../src/utils/constants.js';
import { testKeyPair1, testKeyPair2 } from '../../../fixtures/apply.fixtures.js';
import { publicKeyToAddress } from '../../../../src/utils/helpers.js';
import { ConnectionManagerError } from '../../../../src/core/network/services/ConnectionManager.js';
import { PendingRequestServiceTimeoutError } from '../../../../src/core/network/services/PendingRequestService.js';
import { WalletProvider } from 'trac-wallet';
import { config, overrideConfig } from '../../../helpers/config.js';

async function createWallet(config) {
    return await new WalletProvider(config).fromSecretKey(testKeyPair1.secretKey)
}

const VALIDATOR_KEY = testKeyPair2.publicKey;

const createTransferMessage = (config, wallet) => ({
    type: OperationType.TRANSFER,
    address: wallet.address,
    tro: {
        tx: 'aa'.repeat(32),
        txv: 'bb'.repeat(32),
        in: 'cc'.repeat(32),
        to: publicKeyToAddress(testKeyPair2.publicKey, config),
        am: '00'.repeat(16),
        is: 'dd'.repeat(64),
    },
});

const createConnectionManager = ({
    preferredProtocol = 'v1',
    sendSingleMessage = sinon.stub().resolves(ResultCode.OK),
    sentCount = 0,
    connectedValidators = [VALIDATOR_KEY],
} = {}) => ({
    pickRandomConnectedValidator: sinon.stub().returns(VALIDATOR_KEY),
    pickRandomValidator: sinon.stub().callsFake((validators) => validators[0] ?? null),
    connectedValidators: sinon.stub().returns(connectedValidators),
    getConnection: sinon.stub().returns({
        protocolSession: {
            preferredProtocol,
            supportedProtocols: {
                LEGACY: 'legacy',
                V1: 'v1',
            }
        }
    }),
    sendSingleMessage,
    remove: sinon.stub(),
    incrementSentCount: sinon.stub(),
    getSentCount: sinon.stub().returns(sentCount),
});

hook('setup', () => {
    sinon.stub(console, 'log');
    sinon.stub(console, 'warn');
});

hook('teardown', () => {
    sinon.restore();
});

test('MessageOrchestrator.send returns false for unsupported protocol', async t => {
    const connectionManager = createConnectionManager({ preferredProtocol: 'unknown' });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message, 0);

    t.is(result, false);
    t.is(connectionManager.sendSingleMessage.callCount, 0);
});

test('MessageOrchestrator.send V1 matrix: OK -> SUCCESS', async t => {
    const connectionManager = createConnectionManager({
        sendSingleMessage: sinon.stub().resolves(ResultCode.OK),
        sentCount: 0,
    });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, true);
    t.is(connectionManager.incrementSentCount.callCount, 1);
    t.is(connectionManager.remove.callCount, 0);
});

test('MessageOrchestrator.send V1 matrix: TIMEOUT -> ROTATE', async t => {
    const connectionManager = createConnectionManager({
        sendSingleMessage: sinon.stub().resolves(ResultCode.TIMEOUT),
    });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, false);
    t.is(connectionManager.sendSingleMessage.callCount, 1);
    t.is(connectionManager.remove.callCount, 1);
    t.is(connectionManager.incrementSentCount.callCount, 0);
});

test('MessageOrchestrator.send V1 matrix: TX_ALREADY_PENDING -> NO_ROTATE', async t => {
    const connectionManager = createConnectionManager({
        sendSingleMessage: sinon.stub().resolves(ResultCode.TX_ALREADY_PENDING),
    });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, false);
    t.is(connectionManager.sendSingleMessage.callCount, 1);
    t.is(connectionManager.remove.callCount, 0);
    t.is(connectionManager.incrementSentCount.callCount, 0);
});

test('MessageOrchestrator.send treats TX_ALREADY_EXISTS as success when tx is already visible locally', async t => {
    const connectionManager = createConnectionManager({
        sendSingleMessage: sinon.stub().resolves(ResultCode.TX_ALREADY_EXISTS),
    });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);
    sinon.stub(orchestrator, 'waitForUnsignedState').resolves(true);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, true);
    t.is(connectionManager.sendSingleMessage.callCount, 1);
    t.is(connectionManager.remove.callCount, 0);
    t.is(connectionManager.incrementSentCount.callCount, 0);
});

test('MessageOrchestrator.send treats OPERATION_ALREADY_COMPLETED as success when tx is already visible locally', async t => {
    const connectionManager = createConnectionManager({
        sendSingleMessage: sinon.stub().resolves(ResultCode.OPERATION_ALREADY_COMPLETED),
    });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);
    sinon.stub(orchestrator, 'waitForUnsignedState').resolves(true);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, true);
    t.is(connectionManager.sendSingleMessage.callCount, 1);
    t.is(connectionManager.remove.callCount, 0);
    t.is(connectionManager.incrementSentCount.callCount, 0);
});

test('MessageOrchestrator.send V1 matrix: unknown code -> UNDEFINED', async t => {
    const connectionManager = createConnectionManager({
        sendSingleMessage: sinon.stub().resolves(99999),
    });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, false);
    t.is(connectionManager.sendSingleMessage.callCount, 1);
    t.is(connectionManager.remove.callCount, 1);
});

test('MessageOrchestrator.send removes validator when threshold reached on success', async t => {
    const connectionManager = createConnectionManager({
        sendSingleMessage: sinon.stub().resolves(ResultCode.OK),
        sentCount: config.messageThreshold,
    });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, true);
    t.is(connectionManager.incrementSentCount.callCount, 1);
    t.is(connectionManager.remove.callCount, 1);
});

test('MessageOrchestrator.send retries on ConnectionManagerError without removing validator', async t => {
    const config = overrideConfig({ maxRetries: 2 });
    const sendSingleMessage = sinon.stub();
    sendSingleMessage.onFirstCall().rejects(new ConnectionManagerError('disconnected'));
    sendSingleMessage.onSecondCall().resolves(ResultCode.OK);

    const connectionManager = createConnectionManager({ sendSingleMessage, sentCount: 0 });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, true);
    t.is(sendSingleMessage.callCount, 2);
    t.is(connectionManager.remove.callCount, 0);
    t.is(connectionManager.incrementSentCount.callCount, 1);
});

test('MessageOrchestrator.send retries on generic catch error with remove + retry', async t => {
    const config = overrideConfig({ maxRetries: 2 });
    const sendSingleMessage = sinon.stub();
    sendSingleMessage.onFirstCall().rejects(new Error('response validation failed'));
    sendSingleMessage.onSecondCall().resolves(ResultCode.OK);

    const connectionManager = createConnectionManager({ sendSingleMessage, sentCount: 0 });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, true);
    t.is(sendSingleMessage.callCount, 2);
    t.is(connectionManager.remove.callCount, 1);
    t.is(connectionManager.incrementSentCount.callCount, 1);
});

test('MessageOrchestrator.send max retries guard returns false immediately', async t => {
    const config = overrideConfig({ maxRetries: 1 });
    const connectionManager = createConnectionManager({
        sendSingleMessage: sinon.stub().resolves(ResultCode.OK),
    });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message, 2);

    t.is(result, false);
    t.is(connectionManager.pickRandomConnectedValidator.callCount, 0);
    t.is(connectionManager.sendSingleMessage.callCount, 0);
});

test('MessageOrchestrator.send timeout split: pending timeout rejection goes through catch and retries', async t => {
    const config = overrideConfig({ maxRetries: 2 });
    const sendSingleMessage = sinon.stub();
    sendSingleMessage.onFirstCall().rejects(
        new PendingRequestServiceTimeoutError('req-1', publicKeyToAddress(VALIDATOR_KEY, config), config.pendingRequestTimeout)
    );
    sendSingleMessage.onSecondCall().resolves(ResultCode.OK);

    const connectionManager = createConnectionManager({ sendSingleMessage });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, true);
    t.is(sendSingleMessage.callCount, 2);
    t.is(connectionManager.remove.callCount, 1);
});

test('MessageOrchestrator.send timeout split: TIMEOUT result code stays in then path and does not retry', async t => {
    const config = overrideConfig({ maxRetries: 2 });
    const sendSingleMessage = sinon.stub().resolves(ResultCode.TIMEOUT);
    const connectionManager = createConnectionManager({ sendSingleMessage });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, false);
    t.is(sendSingleMessage.callCount, 1);
    t.is(connectionManager.remove.callCount, 1);
});

test('MessageOrchestrator.send validation split: thrown validation error goes through catch', async t => {
    const config = overrideConfig({ maxRetries: 2 });
    const sendSingleMessage = sinon.stub();
    sendSingleMessage.onFirstCall().rejects(new Error('validator response validation failed'));
    sendSingleMessage.onSecondCall().resolves(ResultCode.OK);

    const connectionManager = createConnectionManager({ sendSingleMessage });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, true);
    t.is(sendSingleMessage.callCount, 2);
    t.is(connectionManager.remove.callCount, 1);
});

test('MessageOrchestrator.send validation split: non-OK result code stays in then and uses policy', async t => {
    const config = overrideConfig({ maxRetries: 2 });
    const sendSingleMessage = sinon.stub().resolves(ResultCode.SCHEMA_VALIDATION_FAILED);
    const connectionManager = createConnectionManager({ sendSingleMessage });
    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, false);
    t.is(sendSingleMessage.callCount, 1);
    t.is(connectionManager.remove.callCount, 1);
});

test('MessageOrchestrator.send legacy path succeeds and increments sent count', async t => {
    const config = overrideConfig({ maxRetries: 0 });
    const sendSingleMessage = sinon.stub().resolves(true);
    const state = { waitForUnsigned: sinon.stub().resolves(true) };
    const connectionManager = createConnectionManager({
        preferredProtocol: 'legacy',
        sendSingleMessage,
        sentCount: 0,
    });
    const orchestrator = new MessageOrchestrator(connectionManager, state, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, true);
    t.is(sendSingleMessage.callCount, 1);
    t.is(state.waitForUnsigned.callCount, 1);
    t.is(connectionManager.incrementSentCount.callCount, 1);
    t.is(connectionManager.remove.callCount, 0);
});

test('MessageOrchestrator.send legacy path false result removes validator and retries', async t => {
    const config = overrideConfig({ maxRetries: 1 });
    const sendSingleMessage = sinon.stub().resolves(true);
    const state = { waitForUnsigned: sinon.stub() };
    state.waitForUnsigned.onFirstCall().resolves(false);
    state.waitForUnsigned.onSecondCall().resolves(true);
    const connectionManager = createConnectionManager({
        preferredProtocol: 'legacy',
        sendSingleMessage,
    });
    const orchestrator = new MessageOrchestrator(connectionManager, state, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, true);
    t.is(sendSingleMessage.callCount, 2);
    t.is(state.waitForUnsigned.callCount, 2);
    t.is(connectionManager.remove.callCount, 1);
});

test('MessageOrchestrator.send legacy path catches send error and retries', async t => {
    const config = overrideConfig({ maxRetries: 1 });
    const sendSingleMessage = sinon.stub();
    sendSingleMessage.onFirstCall().rejects(new Error('legacy send failed'));
    sendSingleMessage.onSecondCall().resolves(true);
    const state = { waitForUnsigned: sinon.stub().resolves(true) };

    const connectionManager = createConnectionManager({
        preferredProtocol: 'legacy',
        sendSingleMessage,
    });
    const orchestrator = new MessageOrchestrator(connectionManager, state, config);
    const wallet = await createWallet(config);
    const message = createTransferMessage(config, wallet);

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, true);
    t.is(sendSingleMessage.callCount, 2);
    t.is(state.waitForUnsigned.callCount, 1);
    t.is(connectionManager.remove.callCount, 0);
});

test('State.waitForUnsigned returns true when state entry appears', async t => {
    const clock = sinon.useFakeTimers({ now: 1 });
    try {
        const state = {
            get: sinon.stub()
                .onFirstCall().resolves(null)
                .onSecondCall().resolves({ tx: 'found' }),
        };

        const pending = State.prototype.waitForUnsigned.call(state, 'tx-hash', 500);
        await clock.tickAsync(450);
        const result = await pending;

        t.is(result, true);
        t.ok(state.get.callCount >= 2);
    } finally {
        clock.restore();
    }
});

test('State.waitForUnsigned returns false on timeout', async t => {
    const clock = sinon.useFakeTimers({ now: 1 });
    try {
        const state = { get: sinon.stub().resolves(null) };

        const pending = State.prototype.waitForUnsigned.call(state, 'tx-hash', 400);
        await clock.tickAsync(1000);
        const result = await pending;

        t.is(result, false);
        t.ok(state.get.callCount >= 1);
    } finally {
        clock.restore();
    }
});

test('MessageOrchestrator.send V1 avoids selecting validator with requester address when possible', async t => {
    const requesterValidatorKey = testKeyPair1.publicKey;
    const otherValidatorKey = testKeyPair2.publicKey;
    const sendSingleMessage = sinon.stub().resolves(ResultCode.OK);

    const connectionManager = createConnectionManager({
        sendSingleMessage,
        connectedValidators: [requesterValidatorKey, otherValidatorKey],
    });
    connectionManager.getConnection = sinon.stub().returns({
        protocolSession: {
            preferredProtocol: 'v1',
            supportedProtocols: {
                LEGACY: 'legacy',
                V1: 'v1',
            }
        }
    });

    const orchestrator = new MessageOrchestrator(connectionManager, { get: async () => null }, config);
    const requesterAddress = publicKeyToAddress(requesterValidatorKey, config);
    const wallet = await createWallet(config);
    const message = {
        ...createTransferMessage(config, wallet),
        address: requesterAddress,
    };

    orchestrator.setWallet(wallet);
    const result = await orchestrator.send(message);

    t.is(result, true);
    t.is(sendSingleMessage.callCount, 1);
    t.is(sendSingleMessage.firstCall.args[1], otherValidatorKey);
});
