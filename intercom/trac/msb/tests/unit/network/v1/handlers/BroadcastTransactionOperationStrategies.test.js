import test from 'brittle';
import b4a from 'b4a';

import {
    createBroadcastTransactionOperationStrategies,
    resolveBroadcastTransactionOperationStrategy
} from '../../../../../src/core/network/protocols/v1/handlers/broadcastTransaction/BroadcastTransactionOperationStrategies.js';
import {OperationType, ResultCode} from '../../../../../src/utils/constants.js';
import {V1ProtocolError} from '../../../../../src/core/network/protocols/v1/V1ProtocolError.js';

const VALID_ADDR = 'trac123z3gfpr2epjwww7ntm3m6ud2fhmq0tvts27p2f5mx3qkecsutlqfys769';
const VALID_TO_ADDR = 'trac1mqktwme8fvklrds4hlhfy6lhmsu9qgfn3c3kuhz7c5zwjt8rc3dqj9tx7h';

const basePayload = () => ({
    tx: b4a.alloc(32),
    txv: b4a.alloc(32),
    in: b4a.alloc(32),
    is: b4a.alloc(64)
});

function createFakeValidator(name, calls) {
    return {
        async validate(decodedTransaction) {
            calls.push({decodedTransaction, validator: name});
        }
    };
}

function createFakeFactory() {
    const calls = [];

    return {
        calls,
        createApplyStateMessageFactory() {
            return {
                buildCompleteAddWriterMessage(...args) {
                    calls.push({args, method: 'buildCompleteAddWriterMessage'});
                    return {method: 'buildCompleteAddWriterMessage'};
                },
                buildCompleteRemoveWriterMessage(...args) {
                    calls.push({args, method: 'buildCompleteRemoveWriterMessage'});
                    return {method: 'buildCompleteRemoveWriterMessage'};
                },
                buildCompleteAdminRecoveryMessage(...args) {
                    calls.push({args, method: 'buildCompleteAdminRecoveryMessage'});
                    return {method: 'buildCompleteAdminRecoveryMessage'};
                },
                buildCompleteTransactionOperationMessage(...args) {
                    calls.push({args, method: 'buildCompleteTransactionOperationMessage'});
                    return {method: 'buildCompleteTransactionOperationMessage'};
                },
                buildCompleteBootstrapDeploymentMessage(...args) {
                    calls.push({args, method: 'buildCompleteBootstrapDeploymentMessage'});
                    return {method: 'buildCompleteBootstrapDeploymentMessage'};
                },
                buildCompleteTransferOperationMessage(...args) {
                    calls.push({args, method: 'buildCompleteTransferOperationMessage'});
                    return {method: 'buildCompleteTransferOperationMessage'};
                }
            };
        }
    };
}

test('resolveBroadcastTransactionOperationStrategy selects and builds all supported transaction types', async t => {
    const validatorCalls = [];
    const factory = createFakeFactory();
    const strategies = createBroadcastTransactionOperationStrategies({
        partialRoleAccessValidator: createFakeValidator('role-access', validatorCalls),
        partialBootstrapDeploymentValidator: createFakeValidator('bootstrap-deployment', validatorCalls),
        partialTransactionValidator: createFakeValidator('transaction', validatorCalls),
        partialTransferValidator: createFakeValidator('transfer', validatorCalls),
        createApplyStateMessageFactory: () => factory.createApplyStateMessageFactory()
    });

    const scenarios = [
        {
            type: OperationType.ADD_WRITER,
            payloadKey: 'rao',
            validator: 'role-access',
            builderMethod: 'buildCompleteAddWriterMessage',
            decodedTransaction: {
                address: VALID_ADDR,
                type: OperationType.ADD_WRITER,
                rao: {
                    ...basePayload(),
                    iw: b4a.alloc(32, 1)
                }
            }
        },
        {
            type: OperationType.REMOVE_WRITER,
            payloadKey: 'rao',
            validator: 'role-access',
            builderMethod: 'buildCompleteRemoveWriterMessage',
            decodedTransaction: {
                address: VALID_ADDR,
                type: OperationType.REMOVE_WRITER,
                rao: {
                    ...basePayload(),
                    iw: b4a.alloc(32, 2)
                }
            }
        },
        {
            type: OperationType.ADMIN_RECOVERY,
            payloadKey: 'rao',
            validator: 'role-access',
            builderMethod: 'buildCompleteAdminRecoveryMessage',
            decodedTransaction: {
                address: VALID_ADDR,
                type: OperationType.ADMIN_RECOVERY,
                rao: {
                    ...basePayload(),
                    iw: b4a.alloc(32, 3)
                }
            }
        },
        {
            type: OperationType.TX,
            payloadKey: 'txo',
            validator: 'transaction',
            builderMethod: 'buildCompleteTransactionOperationMessage',
            decodedTransaction: {
                address: VALID_ADDR,
                type: OperationType.TX,
                txo: {
                    ...basePayload(),
                    iw: b4a.alloc(32, 4),
                    ch: b4a.alloc(32, 5),
                    bs: b4a.alloc(32, 6),
                    mbs: b4a.alloc(32, 7)
                }
            }
        },
        {
            type: OperationType.BOOTSTRAP_DEPLOYMENT,
            payloadKey: 'bdo',
            validator: 'bootstrap-deployment',
            builderMethod: 'buildCompleteBootstrapDeploymentMessage',
            decodedTransaction: {
                address: VALID_ADDR,
                type: OperationType.BOOTSTRAP_DEPLOYMENT,
                bdo: {
                    ...basePayload(),
                    bs: b4a.alloc(32, 8),
                    ic: b4a.alloc(32, 9)
                }
            }
        },
        {
            type: OperationType.TRANSFER,
            payloadKey: 'tro',
            validator: 'transfer',
            builderMethod: 'buildCompleteTransferOperationMessage',
            decodedTransaction: {
                address: VALID_ADDR,
                type: OperationType.TRANSFER,
                tro: {
                    ...basePayload(),
                    to: VALID_TO_ADDR,
                    am: b4a.alloc(16, 10)
                }
            }
        }
    ];

    for (const scenario of scenarios) {
        const strategy = resolveBroadcastTransactionOperationStrategy(scenario.type, strategies);
        const result = await strategy.build(scenario.decodedTransaction);
        const lastValidatorCall = validatorCalls.at(-1);
        const lastFactoryCall = factory.calls.at(-1);

        t.is(strategy.payloadKey, scenario.payloadKey, `${scenario.builderMethod} uses the expected payload key`);
        t.alike(result, {method: scenario.builderMethod}, `${scenario.builderMethod} returns the expected builder output`);
        t.is(lastValidatorCall.validator, scenario.validator, `${scenario.builderMethod} uses the expected validator`);
        t.is(lastValidatorCall.decodedTransaction, scenario.decodedTransaction, `${scenario.builderMethod} validates the incoming payload`);
        t.is(lastFactoryCall.method, scenario.builderMethod, `${scenario.builderMethod} selects the expected completion builder`);
        t.is(lastFactoryCall.args[0], scenario.decodedTransaction.address, `${scenario.builderMethod} forwards the requester address`);
    }
});

test('resolveBroadcastTransactionOperationStrategy rejects unsupported transaction types', t => {
    const strategies = createBroadcastTransactionOperationStrategies({
        partialRoleAccessValidator: createFakeValidator('role-access', []),
        partialBootstrapDeploymentValidator: createFakeValidator('bootstrap-deployment', []),
        partialTransactionValidator: createFakeValidator('transaction', []),
        partialTransferValidator: createFakeValidator('transfer', []),
        createApplyStateMessageFactory: () => createFakeFactory().createApplyStateMessageFactory()
    });

    let error;
    try {
        resolveBroadcastTransactionOperationStrategy(9999, strategies);
    } catch (thrownError) {
        error = thrownError;
    }

    t.ok(error instanceof V1ProtocolError);
    t.is(error.resultCode, ResultCode.TX_INVALID_PAYLOAD);
    t.is(error.message, 'Unsupported transaction type: 9999');
});
