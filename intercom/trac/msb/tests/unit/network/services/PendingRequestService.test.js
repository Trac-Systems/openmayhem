import { test } from 'brittle';
import b4a from 'b4a';
import { v7 as uuidv7 } from 'uuid';
import sinon from 'sinon';
import PendingRequestService from '../../../../src/core/network/services/PendingRequestService.js';
import NetworkMessageBuilder from '../../../../src/messages/network/v1/NetworkMessageBuilder.js';
import { V1ProtocolError } from '../../../../src/core/network/protocols/v1/V1ProtocolError.js';
import { NetworkOperationType, ResultCode } from '../../../../src/utils/constants.js';
import { publicKeyToAddress } from '../../../../src/utils/helpers.js';
import { errorMessageIncludes } from '../../../helpers/regexHelper.js';
import { config } from '../../../helpers/config.js';
import { testKeyPair1, testKeyPair2 } from '../../../fixtures/apply.fixtures.js';
import { WalletProvider } from 'trac-wallet';

const validPeerA = testKeyPair1.publicKey;
const validPeerB = testKeyPair2.publicKey;

async function createWallet() {
    return await new WalletProvider(config).fromSecretKey(testKeyPair1.secretKey)
}
async function buildV1Request({ id = uuidv7() } = {}) {
    const wallet = await createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    await builder
        .setType(NetworkOperationType.LIVENESS_REQUEST)
        .setId(id)
        .setTimestamp()
        .setCapabilities([])
        .buildPayload();
    return builder.getResult();
}

async function buildV1BroadcastRequest({ id = uuidv7(), data = b4a.from('deadbeef', 'hex') } = {}) {
    const wallet = await createWallet();
    const builder = new NetworkMessageBuilder(wallet, config);
    await builder
        .setType(NetworkOperationType.BROADCAST_TRANSACTION_REQUEST)
        .setId(id)
        .setTimestamp()
        .setData(data)
        .setCapabilities([])
        .buildPayload();
    return builder.getResult();
}

test('PendingRequestService registers and resolves v1 request', async t => {
    const service = new PendingRequestService(config);
    const peer = validPeerA;
    const request = await buildV1Request();

    const promise = service.registerPendingRequest(peer, request);
    t.ok(service.has(request.id));

    t.ok(service.resolvePendingRequest(request.id));
    t.is(service.has(request.id), false);
    await promise;

    t.is(service.resolvePendingRequest(request.id), false);
});

test('PendingRequestService rejects and removes pending request', async t => {
    const service = new PendingRequestService(config);
    const peer = validPeerA;
    const request = await buildV1Request();

    const promise = service.registerPendingRequest(peer, request);
    const expectedError = new Error('test');

    t.ok(service.rejectPendingRequest(request.id, expectedError));
    t.is(service.has(request.id), false);

    try {
        await promise;
        t.fail('Expected pending request promise to reject');
    } catch (error) {
        t.is(error, expectedError);
        t.ok(error instanceof Error);
        t.is(error.message, expectedError.message);
    }

    t.is(service.rejectPendingRequest('missing', new Error('missing')), false);
});

test('PendingRequestService preserves typed result-coded errors', async t => {
    const service = new PendingRequestService(config);
    const peer = validPeerA;
    const request = await buildV1Request();

    const promise = service.registerPendingRequest(peer, request);
    const expectedError = new V1ProtocolError(ResultCode.TX_INVALID_PAYLOAD, 'domain-invalid');

    t.ok(service.rejectPendingRequest(request.id, expectedError));

    try {
        await promise;
        t.fail('Expected pending request promise to reject');
    } catch (error) {
        t.is(error, expectedError);
        t.is(error.resultCode, ResultCode.TX_INVALID_PAYLOAD);
        t.is(error.message, 'domain-invalid');
    }
});

test('PendingRequestService throws on duplicate request id', async t => {
    const service = new PendingRequestService(config);
    const peer = validPeerA;
    const request = await buildV1Request();

    const promise = service.registerPendingRequest(peer, request);

    t.exception(
        () => service.registerPendingRequest(peer, request),
        errorMessageIncludes('already exists')
    );

    t.ok(service.resolvePendingRequest(request.id));
    await promise;
});

test('PendingRequestService rejects pending request on timeout', async t => {
    const clock = sinon.useFakeTimers({ now: 1 });
    try {
        const pendingRequestTimeout = 123;
        const service = new PendingRequestService({
            addressPrefix: config.addressPrefix,
            pendingRequestTimeout,
            maxPendingRequestsInPendingRequestsService: 10
        });
        const peer = validPeerA;
        const request = await buildV1Request();

        const promise = service.registerPendingRequest(peer, request);
        promise.catch(() => {});
        t.ok(service.has(request.id));

        await clock.runAllAsync();

        try {
            await promise;
            t.fail('Expected pending request to time out');
        } catch (error) {
            t.is(error.name, 'PendingRequestServiceTimeoutError');
            t.ok(error instanceof Error);
            t.ok(error?.message?.includes(`timed out after ${pendingRequestTimeout} ms`));
            t.ok(error?.message?.includes(publicKeyToAddress(peer, config)));
        }

        t.is(service.has(request.id), false);
    } finally {
        clock.restore();
        sinon.restore();
    }
});

test('PendingRequestService.close rejects all pending requests', async t => {
    const service = new PendingRequestService(config);
    const peer = validPeerA;
    const request1 = await buildV1Request();
    const request2 = await buildV1Request();

    const promise1 = service.registerPendingRequest(peer, request1);
    const promise2 = service.registerPendingRequest(peer, request2);

    service.close();
    t.is(service.has(request1.id), false);
    t.is(service.has(request2.id), false);

    const results = await Promise.allSettled([promise1, promise2]);
    t.is(results[0].status, 'rejected');
    t.ok(results[0]?.reason?.message?.includes(`Pending request ${request1.id} cancelled (shutdown).`));
    t.is(results[1].status, 'rejected');
    t.ok(results[1]?.reason?.message?.includes(`Pending request ${request2.id} cancelled (shutdown).`));
});

test('PendingRequestService rejects all pending requests for a specific peer', async t => {
    const service = new PendingRequestService(config);
    const peerA = validPeerA;
    const peerB = validPeerB;
    const requestA1 = await buildV1Request();
    const requestA2 = await buildV1Request();
    const requestB1 = await buildV1Request();

    const promiseA1 = service.registerPendingRequest(peerA, requestA1);
    const promiseA2 = service.registerPendingRequest(peerA, requestA2);
    const promiseB1 = service.registerPendingRequest(peerB, requestB1);

    const rejectedCount = service.rejectPendingRequestsForPeer(peerA, new Error('peer disconnected'));
    t.is(rejectedCount, 2);
    t.is(service.has(requestA1.id), false);
    t.is(service.has(requestA2.id), false);
    t.is(service.has(requestB1.id), true);

    const results = await Promise.allSettled([promiseA1, promiseA2]);
    t.is(results[0].status, 'rejected');
    t.is(results[0].reason.message, 'peer disconnected');
    t.is(results[1].status, 'rejected');
    t.is(results[1].reason.message, 'peer disconnected');

    t.ok(service.resolvePendingRequest(requestB1.id));
    await promiseB1;
});

test('PendingRequestService stores only transaction data for broadcast requests', async t => {
    const service = new PendingRequestService(config);
    const peer = validPeerA;
    const livenessRequest = await buildV1Request();
    const broadcastRequest = await buildV1BroadcastRequest();

    const livenessPromise = service.registerPendingRequest(peer, livenessRequest);
    const broadcastPromise = service.registerPendingRequest(peer, broadcastRequest);

    const livenessEntry = service.getPendingRequest(livenessRequest.id);
    const broadcastEntry = service.getPendingRequest(broadcastRequest.id);

    t.is(livenessEntry.requestTxData, null);
    t.ok(b4a.isBuffer(broadcastEntry.requestTxData));
    t.alike(broadcastEntry.requestTxData, broadcastRequest.broadcast_transaction_request.data);
    t.is(Object.prototype.hasOwnProperty.call(livenessEntry, 'requestMessage'), false);
    t.is(Object.prototype.hasOwnProperty.call(broadcastEntry, 'requestMessage'), false);

    t.ok(service.resolvePendingRequest(livenessRequest.id));
    t.ok(service.resolvePendingRequest(broadcastRequest.id));
    await Promise.all([livenessPromise, broadcastPromise]);
});

test('PendingRequestService.isProbePending matches peer and liveness type', async t => {
    const service = new PendingRequestService(config);
    const peerA = validPeerA;
    const peerB = validPeerB;

    t.is(service.isProbePending(peerA), false);
    t.is(service.isProbePending(peerB), false);

    const broadcastRequestA = await buildV1BroadcastRequest();
    const livenessRequestA = await buildV1Request();
    const livenessRequestB = await buildV1Request();

    const broadcastPromiseA = service.registerPendingRequest(peerA, broadcastRequestA);
    t.is(service.isProbePending(peerA), false);

    const livenessPromiseA = service.registerPendingRequest(peerA, livenessRequestA);
    const livenessPromiseB = service.registerPendingRequest(peerB, livenessRequestB);

    t.is(service.isProbePending(peerA), true);
    t.is(service.isProbePending(peerB), true);

    t.ok(service.resolvePendingRequest(livenessRequestA.id));
    await livenessPromiseA;
    t.is(service.isProbePending(peerA), false);
    t.is(service.isProbePending(peerB), true);

    t.ok(service.resolvePendingRequest(livenessRequestB.id));
    await livenessPromiseB;
    t.is(service.isProbePending(peerB), false);

    t.ok(service.resolvePendingRequest(broadcastRequestA.id));
    await broadcastPromiseA;
});

test('PendingRequestService enforces global pending request limit', async t => {
    const service = new PendingRequestService({
        addressPrefix: config.addressPrefix,
        pendingRequestTimeout: config.pendingRequestTimeout,
        maxPendingRequestsInPendingRequestsService: 3
    });
    const peer = validPeerA;

    const request0 = await buildV1Request({ id: 'limit-0' });
    const request1 = await buildV1Request({ id: 'limit-1' });
    const request2 = await buildV1Request({ id: 'limit-2' });
    const overflowRequest = await buildV1Request({ id: 'limit-overflow' });

    service.registerPendingRequest(peer, request0).catch(() => {});
    service.registerPendingRequest(peer, request1).catch(() => {});
    service.registerPendingRequest(peer, request2).catch(() => {});

    t.exception(
        () => service.registerPendingRequest(peer, overflowRequest),
        errorMessageIncludes('Maximum number of pending requests reached.')
    );

    service.close();
});

test('PendingRequestService.stopPendingRequestTimeout stops timeout and handles missing id', async t => {
    const clock = sinon.useFakeTimers({ now: 1 });
    try {
        const service = new PendingRequestService(config);
        const peer = validPeerA;
        const request = await buildV1Request();

        const promise = service.registerPendingRequest(peer, request);
        t.is(service.stopPendingRequestTimeout('missing-id'), false);
        t.is(service.stopPendingRequestTimeout(request.id), true);
        t.is(service.getPendingRequest(request.id)?.timeoutId, null);

        let settled = false;
        promise.then(
            () => { settled = true; },
            () => { settled = true; }
        );

        await clock.runAllAsync();
        await Promise.resolve();
        t.is(settled, false);

        t.ok(service.resolvePendingRequest(request.id));
        await promise;
    } finally {
        clock.restore();
        sinon.restore();
    }
});


test('PendingRequestService.getPendingRequest returns null for missing id', t => {
    const service = new PendingRequestService(config);
    t.is(service.getPendingRequest('missing-id'), null);
});


test('PendingRequestService rejects invalid registerPendingRequest input', async t => {
    const service = new PendingRequestService(config);
    const peer = validPeerA;
    const validLivenessRequest = await buildV1Request({ id: 'invalid-peer' });

    t.exception(
        () => service.registerPendingRequest(peer, 'not-an-object'),
        errorMessageIncludes('Pending request message must be an object.')
    );

    t.exception(
        () => service.registerPendingRequest(peer, {
            id: '',
            type: NetworkOperationType.LIVENESS_REQUEST
        }),
        errorMessageIncludes('Pending request ID must be a non-empty string.')
    );

    t.exception(
        () => service.registerPendingRequest(peer, {
            id: 'invalid-type',
            type: 'LIVENESS_REQUEST'
        }),
        errorMessageIncludes('Unsupported pending request type.')
    );

    t.exception(
        () => service.registerPendingRequest(peer, {
            id: 'unsupported-type',
            type: 999
        }),
        errorMessageIncludes('Unsupported pending request type.')
    );

    t.exception(
        () => service.registerPendingRequest('deadbeef', validLivenessRequest),
        errorMessageIncludes('Invalid peer public key. Expected 32-byte hex string.')
    );

    t.is(service.getPendingRequest('unsupported-type'), null);
    t.is(service.getPendingRequest('invalid-type'), null);
});

test('PendingRequestService.rejectPendingRequest falls back to Unexpected error message', async t => {
    const service = new PendingRequestService(config);
    const peer = validPeerA;
    const request = await buildV1Request();
    const promise = service.registerPendingRequest(peer, request);

    t.ok(service.rejectPendingRequest(request.id, {}));

    try {
        await promise;
        t.fail('Expected pending request promise to reject');
    } catch (error) {
        t.ok(error instanceof Error);
        t.is(error.message, 'Unexpected error');
    }
});

test('PendingRequestService.close catches reject errors and continues cleanup', async t => {
    const service = new PendingRequestService(config);
    const peer = validPeerA;
    const request = await buildV1Request();
    const promise = service.registerPendingRequest(peer, request);
    const entry = service.getPendingRequest(request.id);
    const originalReject = entry.reject;
    const logs = [];
    const originalConsoleError = console.error;

    console.error = (...args) => {
        logs.push(args);
    };
    t.teardown(() => {
        console.error = originalConsoleError;
    });

    entry.reject = error => {
        originalReject(error);
        throw new Error('forced reject failure');
    };

    service.close();
    t.is(service.has(request.id), false);
    t.is(logs.length, 1);
    t.ok(String(logs[0][0]).includes('PendingRequestService.close: failed to reject pending request'));

    const result = await Promise.allSettled([promise]);
    t.is(result[0].status, 'rejected');
    t.ok(result[0]?.reason?.message?.includes(`Pending request ${request.id} cancelled (shutdown).`));
});

test('PendingRequestService throws when registerPendingRequest receives null message', t => {
    const service = new PendingRequestService(config);
    const peer = validPeerA;
    try {
        service.registerPendingRequest(peer, null);
        t.fail('Expected registerPendingRequest to throw for null message');
    } catch (error) {
        t.ok(error?.message?.includes('Pending request message must be an object.'));
    }
});
