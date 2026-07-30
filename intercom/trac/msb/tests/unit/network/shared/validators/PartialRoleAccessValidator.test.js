import test from 'brittle';
import b4a from 'b4a';

import PartialRoleAccessValidator from '../../../../../src/core/network/protocols/shared/validators/PartialRoleAccessValidator.js';
import { ResultCode, OperationType } from '../../../../../src/utils/constants.js';
import { bufferToBigInt } from '../../../../../src/utils/amountSerialization.js';
import { FEE } from '../../../../../src/core/state/utils/transaction.js';
import { $TNK } from '../../../../../src/core/state/utils/balance.js';
import { addressToBuffer } from '../../../../../src/core/state/utils/address.js';
import { bigIntToBuffer } from '../../../../../src/utils/buffer.js';
import { config } from '../../../../helpers/config.js';
import {
    createNodeEntry,
    createState,
    getWalletSet,
    buildRoleAccessPayload,
    expectSharedValidatorError
} from '../../utils/sharedValidatorTestUtils.js';

const FEE_BIGINT = bufferToBigInt(FEE);

test('PartialRoleAccessValidator.validate accepts add-writer, remove-writer, and admin-recovery happy paths', async t => {
    const { requester, validator } = await getWalletSet();

    const addWriterPayload = await buildRoleAccessPayload(OperationType.ADD_WRITER, requester);
    const addWriterState = createState({
        signedEntries: new Map([[requester.address, createNodeEntry({ wk: addWriterPayload.rao.iw, balance: bigIntToBuffer(FEE_BIGINT * 11n), isWhitelisted: true })]]),
        unsignedEntries: new Map([[requester.address, createNodeEntry({ wk: addWriterPayload.rao.iw, balance: bigIntToBuffer(FEE_BIGINT * 11n), isWhitelisted: true })]])
    });
    await new PartialRoleAccessValidator(addWriterState, validator.address, config).validate(addWriterPayload);
    t.pass('add writer happy path');

    const removeWriterPayload = await buildRoleAccessPayload(OperationType.REMOVE_WRITER, requester);
    const removeWriterState = createState({
        signedEntries: new Map([[requester.address, createNodeEntry({ wk: removeWriterPayload.rao.iw, balance: $TNK(100n), isWhitelisted: true, isWriter: true })]]),
        unsignedEntries: new Map([[requester.address, createNodeEntry({ wk: removeWriterPayload.rao.iw, balance: $TNK(100n), isWhitelisted: true, isWriter: true })]])
    });
    await new PartialRoleAccessValidator(removeWriterState, validator.address, config).validate(removeWriterPayload);
    t.pass('remove writer happy path');

    const recoveryPayload = await buildRoleAccessPayload(OperationType.ADMIN_RECOVERY, requester, undefined, b4a.alloc(32, 0x77));
    const recoveryState = createState({
        adminEntry: { address: requester.address, wk: b4a.alloc(32, 0x66) },
        signedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n), isWhitelisted: true, isWriter: true })]]),
        unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n), isWhitelisted: true, isWriter: true })]])
    });
    await new PartialRoleAccessValidator(recoveryState, validator.address, config).validate(recoveryPayload);
    t.pass('admin recovery happy path');
});

test('PartialRoleAccessValidator.validate enforces add-writer state rules and exact balance boundary', async t => {
    const { requester, validator, alternate } = await getWalletSet();
    const payload = await buildRoleAccessPayload(OperationType.ADD_WRITER, requester);

    const exactBalanceState = createState({
        signedEntries: new Map([[requester.address, createNodeEntry({ wk: payload.rao.iw, balance: bigIntToBuffer(FEE_BIGINT * 11n), isWhitelisted: true })]]),
        unsignedEntries: new Map([[requester.address, createNodeEntry({ wk: payload.rao.iw, balance: bigIntToBuffer(FEE_BIGINT * 11n), isWhitelisted: true })]])
    });
    await new PartialRoleAccessValidator(exactBalanceState, validator.address, config).validate(payload);
    t.pass('exact 11x fee balance accepted');

    await expectSharedValidatorError(
        t,
        () => new PartialRoleAccessValidator(
            createState({
                signedEntries: new Map([[requester.address, createNodeEntry({ wk: payload.rao.iw, balance: $TNK(100n), isWhitelisted: true, isWriter: true })]]),
                unsignedEntries: new Map([[requester.address, createNodeEntry({ wk: payload.rao.iw, balance: $TNK(100n), isWhitelisted: true, isWriter: true })]])
            }),
            validator.address,
            config
        ).validate(payload),
        ResultCode.ROLE_NODE_ALREADY_WRITER,
        'already a writer'
    );

    await expectSharedValidatorError(
        t,
        () => new PartialRoleAccessValidator(
            createState({
                signedEntries: new Map([[requester.address, createNodeEntry({ wk: payload.rao.iw, balance: $TNK(100n), isWhitelisted: false })]]),
                unsignedEntries: new Map([[requester.address, createNodeEntry({ wk: payload.rao.iw, balance: $TNK(100n), isWhitelisted: false })]])
            }),
            validator.address,
            config
        ).validate(payload),
        ResultCode.ROLE_NODE_NOT_WHITELISTED,
        'not whitelisted'
    );

    await expectSharedValidatorError(
        t,
        () => new PartialRoleAccessValidator(
            createState({
                signedEntries: new Map([[requester.address, createNodeEntry({ wk: payload.rao.iw, balance: bigIntToBuffer(FEE_BIGINT * 11n - 1n), isWhitelisted: true })]]),
                unsignedEntries: new Map([[requester.address, createNodeEntry({ wk: payload.rao.iw, balance: bigIntToBuffer(FEE_BIGINT * 11n - 1n), isWhitelisted: true })]])
            }),
            validator.address,
            config
        ).validate(payload),
        ResultCode.ROLE_INSUFFICIENT_FEE_BALANCE,
        'Insufficient requester balance'
    );

    await expectSharedValidatorError(
        t,
        () => new PartialRoleAccessValidator(
            createState({
                signedEntries: new Map([[requester.address, createNodeEntry({ wk: payload.rao.iw, balance: bigIntToBuffer(FEE_BIGINT * 11n), isWhitelisted: true })]]),
                unsignedEntries: new Map([[requester.address, createNodeEntry({ wk: payload.rao.iw, balance: bigIntToBuffer(FEE_BIGINT * 11n), isWhitelisted: true })]]),
                registeredWriterKeys: new Map([[payload.rao.iw.toString('hex'), addressToBuffer(alternate.address, config.addressPrefix)]])
            }),
            validator.address,
            config
        ).validate(payload),
        ResultCode.ROLE_INVALID_WRITER_KEY,
        'Invalid writer key'
    );
});

test('PartialRoleAccessValidator.validate enforces remove-writer and admin-recovery state guards', async t => {
    const { requester, validator } = await getWalletSet();

    const removeWriterPayload = await buildRoleAccessPayload(OperationType.REMOVE_WRITER, requester);
    await expectSharedValidatorError(
        t,
        () => new PartialRoleAccessValidator(
            createState({
                signedEntries: new Map([[requester.address, createNodeEntry({ wk: removeWriterPayload.rao.iw, balance: $TNK(100n), isWhitelisted: true, isWriter: false })]]),
                unsignedEntries: new Map([[requester.address, createNodeEntry({ wk: removeWriterPayload.rao.iw, balance: $TNK(100n), isWhitelisted: true, isWriter: false })]])
            }),
            validator.address,
            config
        ).validate(removeWriterPayload),
        ResultCode.ROLE_NODE_NOT_WRITER,
        'not a writer'
    );

    await expectSharedValidatorError(
        t,
        () => new PartialRoleAccessValidator(
            createState({
                signedEntries: new Map([[requester.address, createNodeEntry({ wk: removeWriterPayload.rao.iw, balance: $TNK(100n), isWhitelisted: true, isWriter: true, isIndexer: true })]]),
                unsignedEntries: new Map([[requester.address, createNodeEntry({ wk: removeWriterPayload.rao.iw, balance: $TNK(100n), isWhitelisted: true, isWriter: true, isIndexer: true })]])
            }),
            validator.address,
            config
        ).validate(removeWriterPayload),
        ResultCode.ROLE_NODE_IS_INDEXER,
        'is an indexer'
    );

    const recoveryPayload = await buildRoleAccessPayload(OperationType.ADMIN_RECOVERY, requester, undefined, b4a.alloc(32, 0x99));
    await expectSharedValidatorError(
        t,
        () => new PartialRoleAccessValidator(
            createState({
                signedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n), isWhitelisted: true, isWriter: true })]]),
                unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n), isWhitelisted: true, isWriter: true })]])
            }),
            validator.address,
            config
        ).validate(recoveryPayload),
        ResultCode.ROLE_ADMIN_ENTRY_MISSING,
        'Admin entry does not exist'
    );

    await expectSharedValidatorError(
        t,
        () => new PartialRoleAccessValidator(
            createState({
                adminEntry: { address: requester.address, wk: recoveryPayload.rao.iw },
                signedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n), isWhitelisted: true, isWriter: true })]]),
                unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n), isWhitelisted: true, isWriter: true })]])
            }),
            validator.address,
            config
        ).validate(recoveryPayload),
        ResultCode.ROLE_INVALID_RECOVERY_CASE,
        'not a valid recovery case'
    );
});

test('PartialRoleAccessValidator direct helper checks cover missing entries and unknown operations', async t => {
    const { requester, validator } = await getWalletSet();
    const payload = await buildRoleAccessPayload(OperationType.ADD_WRITER, requester);
    const validatorInstance = new PartialRoleAccessValidator(createState(), validator.address, config);

    await expectSharedValidatorError(
        t,
        () => validatorInstance.validateWriterKey(payload),
        ResultCode.REQUESTER_NOT_FOUND,
        'Node entry not found'
    );

    await expectSharedValidatorError(
        t,
        () => validatorInstance.validateRequesterBalanceForAddWriterOperation(payload),
        ResultCode.REQUESTER_NOT_FOUND,
        'Requester address not found'
    );

    await expectSharedValidatorError(
        t,
        () => validatorInstance.validateRequesterBalanceForAddWriterOperation(payload, true),
        ResultCode.REQUESTER_NOT_FOUND,
        'Requester address not found'
    );

    await expectSharedValidatorError(
        t,
        () => validatorInstance.isRequesterAllowedToChangeRole(payload),
        ResultCode.ROLE_NODE_ENTRY_NOT_FOUND,
        'entry does not exist'
    );

    const removeWriterPayload = await buildRoleAccessPayload(OperationType.REMOVE_WRITER, requester);
    await expectSharedValidatorError(
        t,
        () => validatorInstance.isRequesterAllowedToChangeRole(removeWriterPayload),
        ResultCode.ROLE_NODE_ENTRY_NOT_FOUND,
        'entry does not exist'
    );

    const unknownPayload = { ...payload, type: 999999 };
    await expectSharedValidatorError(
        t,
        () => validatorInstance.isRequesterAllowedToChangeRole(unknownPayload),
        ResultCode.ROLE_UNKNOWN_OPERATION,
        'Unknown role access operation type'
    );
});

test('PartialRoleAccessValidator.validateWriterKey accepts a registered writer key owned by requester', async t => {
    const { requester, validator } = await getWalletSet();
    const payload = await buildRoleAccessPayload(OperationType.ADD_WRITER, requester);
    const state = createState({
        signedEntries: new Map([[requester.address, createNodeEntry({ wk: payload.rao.iw, balance: $TNK(100n), isWhitelisted: true })]]),
        registeredWriterKeys: new Map([[payload.rao.iw.toString('hex'), payload.address]])
    });

    await new PartialRoleAccessValidator(state, validator.address, config).validateWriterKey(payload);
    t.pass();
});
