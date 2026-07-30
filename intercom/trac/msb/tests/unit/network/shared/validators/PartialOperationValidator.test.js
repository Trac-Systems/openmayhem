import test from 'brittle';
import b4a from 'b4a';
import tracCryptoApi from 'trac-crypto-api';

import PartialOperationValidator from '../../../../../src/core/network/protocols/shared/validators/PartialOperationValidator.js';
import { ResultCode, OperationType } from '../../../../../src/utils/constants.js';
import { bufferToBigInt } from '../../../../../src/utils/amountSerialization.js';
import { FEE } from '../../../../../src/core/state/utils/transaction.js';
import { $TNK } from '../../../../../src/core/state/utils/balance.js';
import { bigIntToBuffer } from '../../../../../src/utils/buffer.js';
import { config } from '../../../../helpers/config.js';
import {
    createNodeEntry,
    createState,
    getWalletSet,
    buildRoleAccessPayload,
    buildBootstrapDeploymentPayload,
    buildTransactionPayload,
    buildTransferPayload,
    expectSharedValidatorError,
    getPayloadTxHex
} from '../../utils/sharedValidatorTestUtils.js';

const FEE_BIGINT = bufferToBigInt(FEE);

test('PartialOperationValidator.validate rejects direct base-class usage', async t => {
    const { requester, validator } = await getWalletSet();
    const state = createState();
    const validatorInstance = new PartialOperationValidator(state, validator.address, config);
    const payload = await buildTransferPayload(requester, requester.address, bigIntToBuffer(1n));

    await expectSharedValidatorError(
        t,
        () => validatorInstance.validate(payload),
        ResultCode.UNEXPECTED_ERROR,
        'must be implemented'
    );
});

test('PartialOperationValidator.isPayloadSchemaValid accepts supported payload shapes', async t => {
    const { requester, recipient, validator } = await getWalletSet();
    const state = createState();
    const validatorInstance = new PartialOperationValidator(state, validator.address, config);

    const payloads = [
        await buildRoleAccessPayload(OperationType.ADD_WRITER, requester),
        await buildBootstrapDeploymentPayload(requester),
        await buildTransactionPayload(requester),
        await buildTransferPayload(requester, recipient.address, bigIntToBuffer(1n))
    ];

    for (const payload of payloads) {
        validatorInstance.isPayloadSchemaValid(payload);
        t.pass(`schema valid for operation type ${payload.type}`);
    }
});

test('PartialOperationValidator.isPayloadSchemaValid rejects missing or unknown types', async t => {
    const { requester, recipient, validator } = await getWalletSet();
    const state = createState();
    const validatorInstance = new PartialOperationValidator(state, validator.address, config);
    const validPayload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(1n));

    await expectSharedValidatorError(
        t,
        () => Promise.resolve(validatorInstance.isPayloadSchemaValid({})),
        ResultCode.TX_INVALID_PAYLOAD,
        'type is missing'
    );

    await expectSharedValidatorError(
        t,
        () => Promise.resolve(validatorInstance.isPayloadSchemaValid({ ...validPayload, type: 999999 })),
        ResultCode.OPERATION_TYPE_UNKNOWN,
        'Unknown operation type'
    );

    const invalidShapePayload = { ...validPayload };
    delete invalidShapePayload.tro.am;
    await expectSharedValidatorError(
        t,
        () => Promise.resolve(validatorInstance.isPayloadSchemaValid(invalidShapePayload)),
        ResultCode.SCHEMA_VALIDATION_FAILED,
        'Payload is invalid'
    );
});

test('PartialOperationValidator.validateRequesterAddress rejects malformed address buffers', async t => {
    const { validator } = await getWalletSet();
    const state = createState();
    const validatorInstance = new PartialOperationValidator(state, validator.address, config);
    const payload = {
        type: OperationType.TRANSFER,
        address: b4a.alloc(config.addressLength - 1),
        tro: {
            tx: b4a.alloc(32),
            txv: b4a.alloc(32),
            in: b4a.alloc(32),
            to: b4a.alloc(config.addressLength),
            am: b4a.alloc(16),
            is: b4a.alloc(64)
        }
    };

    await expectSharedValidatorError(
        t,
        () => Promise.resolve(validatorInstance.validateRequesterAddress(payload)),
        ResultCode.REQUESTER_ADDRESS_INVALID,
        'Invalid requesting address'
    );
});

test('PartialOperationValidator.validateRequesterAddress rejects invalid requester public keys', async t => {
    const { requester, recipient, validator } = await getWalletSet();
    const payload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(1n));
    const validatorInstance = new PartialOperationValidator(createState(), validator.address, config);
    const originalDecodeSafe = tracCryptoApi.address.decodeSafe;

    tracCryptoApi.address.decodeSafe = address => (
        address === requester.address ? b4a.alloc(31, 0x01) : originalDecodeSafe(address)
    );
    t.teardown(() => {
        tracCryptoApi.address.decodeSafe = originalDecodeSafe;
    });

    await expectSharedValidatorError(
        t,
        () => Promise.resolve(validatorInstance.validateRequesterAddress(payload)),
        ResultCode.REQUESTER_PUBLIC_KEY_INVALID,
        'Invalid requesting public key'
    );
});

test('PartialOperationValidator.validateSignature accepts signed payloads and rejects tampering', async t => {
    const { requester, recipient, validator } = await getWalletSet();
    const payload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(1n));
    const txHex = getPayloadTxHex(payload);
    const state = createState({
        unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(10n) })]]),
        txEntries: new Map([[txHex, null]])
    });
    const validatorInstance = new PartialOperationValidator(state, validator.address, config);

    await validatorInstance.validateSignature(payload);
    t.pass('valid signature accepted');

    const mismatchedHashPayload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(1n));
    mismatchedHashPayload.tro.tx = b4a.alloc(32, 0xaa);
    await expectSharedValidatorError(
        t,
        () => validatorInstance.validateSignature(mismatchedHashPayload),
        ResultCode.TX_HASH_MISMATCH,
        'does not match incoming transaction'
    );

    const invalidSignaturePayload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(1n));
    invalidSignaturePayload.tro.is = b4a.alloc(64);
    await expectSharedValidatorError(
        t,
        () => validatorInstance.validateSignature(invalidSignaturePayload),
        ResultCode.TX_SIGNATURE_INVALID,
        'Invalid signature'
    );

    const unknownOperationPayload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(1n));
    unknownOperationPayload.type = 999999;
    unknownOperationPayload.undefined = { is: unknownOperationPayload.tro.is };
    await expectSharedValidatorError(
        t,
        () => validatorInstance.validateSignature(unknownOperationPayload),
        ResultCode.OPERATION_TYPE_UNKNOWN,
        'Unknown operation type'
    );
});

test('PartialOperationValidator common state checks cover tx validity, uniqueness, completion and balance edges', async t => {
    const { requester, recipient, validator } = await getWalletSet();
    const payload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(1n));
    const txHex = getPayloadTxHex(payload);
    const validatorInstance = new PartialOperationValidator(createState(), validator.address, config);

    await expectSharedValidatorError(
        t,
        () => new PartialOperationValidator(
            createState({ txValidity: b4a.alloc(32, 0x99) }),
            validator.address,
            config
        ).validateTransactionValidity(payload),
        ResultCode.TX_EXPIRED,
        'expired'
    );

    await expectSharedValidatorError(
        t,
        () => new PartialOperationValidator(
            createState({ txEntries: new Map([[txHex, b4a.from('exists')]]) }),
            validator.address,
            config
        ).validateTransactionUniqueness(payload),
        ResultCode.TX_ALREADY_EXISTS,
        'already exists'
    );

    const completedPayload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(1n));
    completedPayload.tro.va = b4a.alloc(config.addressLength);
    await expectSharedValidatorError(
        t,
        () => Promise.resolve(validatorInstance.isOperationNotCompleted(completedPayload)),
        ResultCode.OPERATION_ALREADY_COMPLETED,
        'must be undefined'
    );

    const exactFeeState = createState({
        unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: bigIntToBuffer(FEE_BIGINT) })]]),
        signedEntries: new Map([[requester.address, createNodeEntry({ balance: bigIntToBuffer(FEE_BIGINT) })]])
    });
    const exactFeeValidator = new PartialOperationValidator(exactFeeState, validator.address, config);
    await exactFeeValidator.validateRequesterBalance(payload);
    await exactFeeValidator.validateRequesterBalance(payload, true);
    t.pass('exact fee balance accepted for signed and unsigned requester');

    await expectSharedValidatorError(
        t,
        () => new PartialOperationValidator(createState(), validator.address, config).validateRequesterBalance(payload),
        ResultCode.REQUESTER_NOT_FOUND,
        'not found'
    );

    await expectSharedValidatorError(
        t,
        () => new PartialOperationValidator(
            createState({
                unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: bigIntToBuffer(FEE_BIGINT - 1n) })]])
            }),
            validator.address,
            config
        ).validateRequesterBalance(payload),
        ResultCode.INSUFFICIENT_FEE_BALANCE,
        'Insufficient balance'
    );
});

test('PartialOperationValidator bootstrap and self-validation guards reject invalid edge cases', async t => {
    const { requester, validator } = await getWalletSet();
    const txPayload = await buildTransactionPayload(requester, undefined, { externalBootstrap: config.bootstrap });

    await expectSharedValidatorError(
        t,
        () => Promise.resolve(new PartialOperationValidator(createState(), validator.address, config).validateSubnetworkBootstrapEquality(txPayload)),
        ResultCode.EXTERNAL_BOOTSTRAP_EQUALS_MSB_BOOTSTRAP,
        'same as MSB bootstrap'
    );

    const rolePayload = await buildRoleAccessPayload(OperationType.ADD_WRITER, requester);
    await expectSharedValidatorError(
        t,
        () => Promise.resolve(new PartialOperationValidator(createState(), requester.address, config).validateNoSelfValidation(rolePayload)),
        ResultCode.SELF_VALIDATION_FORBIDDEN,
        'cannot be the same'
    );

    new PartialOperationValidator(createState(), null, config).validateNoSelfValidation(rolePayload);
    t.pass('self-validation check ignored when validator address is unset');
});
