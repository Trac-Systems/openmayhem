import { test } from 'brittle';
import b4a from 'b4a';
import { config } from '../../../helpers/config.js';
import { testKeyPair1, testKeyPair2 } from '../../../fixtures/apply.fixtures.js';
import V1BroadcastTransactionOperationHandler from '../../../../src/core/network/protocols/v1/handlers/V1BroadcastTransactionOperationHandler.js';
import PartialTransactionValidator from '../../../../src/core/network/protocols/shared/validators/PartialTransactionValidator.js';
import { TransactionPoolFullError } from '../../../../src/core/network/services/TransactionPoolService.js';
import { V1ProtocolError } from '../../../../src/core/network/protocols/v1/V1ProtocolError.js';
import { applyStateMessageFactory } from '../../../../src/messages/state/applyStateMessageFactory.js';
import { ResultCode } from '../../../../src/utils/constants.js';
import { WalletProvider } from 'trac-wallet';

async function createWallet(keyPair) {
    return await new WalletProvider(config).fromSecretKey(keyPair.secretKey)
}

test('V1BroadcastTransactionOperationHandler dispatchTransaction does not emit unhandledRejection when enqueue fails after pending commit registration', async t => {
    const originalValidate = PartialTransactionValidator.prototype.validate;
    PartialTransactionValidator.prototype.validate = async () => true;

    try {
        const txPoolService = {
            addTransaction() {
                throw new TransactionPoolFullError(1);
            }
        };

        const pendingRejectors = new Map();
        const transactionCommitService = {
            registerPendingCommit(txHash) {
                return new Promise((resolve, reject) => {
                    pendingRejectors.set(txHash, reject);
                });
            },
            rejectPendingCommit(txHash, error) {
                const reject = pendingRejectors.get(txHash);
                if (!reject) return false;
                pendingRejectors.delete(txHash);
                reject(error);
                return true;
            }
        };

        const validatorWallet = await createWallet(testKeyPair1);
        const requesterWallet = await createWallet(testKeyPair2);
        const externalBootstrap = b4a.alloc(32, 0x11);

        const decodedTransaction = await applyStateMessageFactory(requesterWallet, config)
            .buildPartialTransactionOperationMessage(
                requesterWallet.address,
                b4a.alloc(32, 0x22),
                b4a.alloc(32, 0x33),
                b4a.alloc(32, 0x44),
                externalBootstrap,
                config.bootstrap,
                'buffer'
            );

        const handler = new V1BroadcastTransactionOperationHandler(
            {},
            validatorWallet,
            { v1HandleRateLimit() {} },
            txPoolService,
            { getPendingRequest() { return null; } },
            transactionCommitService,
            config
        );

        let unhandled = null;
        const onUnhandled = (error) => {
            unhandled = error;
        };

        const detachUnhandled = (() => {
            const proc = globalThis.process;
            if (proc?.once && proc?.removeListener) {
                proc.once('unhandledRejection', onUnhandled);
                return () => proc.removeListener('unhandledRejection', onUnhandled);
            }
            if (typeof globalThis.addEventListener === 'function') {
                const listener = (event) => onUnhandled(event?.reason ?? event);
                globalThis.addEventListener('unhandledrejection', listener);
                return () => globalThis.removeEventListener('unhandledrejection', listener);
            }
            if ('onunhandledrejection' in globalThis) {
                const previous = globalThis.onunhandledrejection;
                globalThis.onunhandledrejection = (event) => {
                    onUnhandled(event?.reason ?? event);
                    if (typeof previous === 'function') {
                        previous(event);
                    }
                };
                return () => {
                    globalThis.onunhandledrejection = previous;
                };
            }
            return null;
        })();
        try {
            let thrown = null;
            try {
                await handler.dispatchTransaction(decodedTransaction);
            } catch (error) {
                thrown = error;
            }

            t.ok(thrown, 'dispatchTransaction should throw when enqueue fails');
            t.ok(thrown instanceof V1ProtocolError, 'should map tx pool full to V1ProtocolError');
            t.is(thrown.resultCode, ResultCode.NODE_OVERLOADED);
            await new Promise(resolve => setImmediate(resolve));
        } finally {
            if (detachUnhandled) {
                detachUnhandled();
            }
        }

        t.absent(unhandled, 'should not emit unhandledRejection');
    } finally {
        PartialTransactionValidator.prototype.validate = originalValidate;
    }
});
