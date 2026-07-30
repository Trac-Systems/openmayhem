import test from 'brittle';
import b4a from 'b4a';
import tracCryptoApi from 'trac-crypto-api';

import PartialTransferValidator from '../../../../../src/core/network/protocols/shared/validators/PartialTransferValidator.js';
import { ResultCode } from '../../../../../src/utils/constants.js';
import { bigIntToBuffer } from '../../../../../src/utils/buffer.js';
import { config } from '../../../../helpers/config.js';
import {
    createNodeEntry,
    createState,
    getWalletSet,
    buildTransferPayload,
    createZeroPublicKeyAddress,
    expectSharedValidatorError
} from '../../utils/sharedValidatorTestUtils.js';

const MAX_AMOUNT = BigInt('0xffffffffffffffffffffffffffffffff');

test('PartialTransferValidator.validate accepts self-transfer max amount and recipient transfer exact-balance boundary', async t => {
    const { requester, validator, recipient } = await getWalletSet();

    const selfPayload = await buildTransferPayload(requester, requester.address, bigIntToBuffer(MAX_AMOUNT));
    const selfState = createState({
        unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: bigIntToBuffer(1n) })]])
    });
    const selfValidator = new PartialTransferValidator(selfState, validator.address, config);
    selfValidator.fee = 1n;
    await selfValidator.validate(selfPayload);
    t.pass('self transfer accepts max amount when fee is covered');

    const recipientPayload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(25n));
    const boundaryState = createState({
        unsignedEntries: new Map([
            [requester.address, createNodeEntry({ balance: bigIntToBuffer(26n) })],
            [recipient.address, createNodeEntry({ balance: bigIntToBuffer(10n) })]
        ])
    });
    const boundaryValidator = new PartialTransferValidator(boundaryState, validator.address, config);
    boundaryValidator.fee = 1n;
    await boundaryValidator.validate(recipientPayload);
    t.pass('sender exact amount + fee passes');

    const missingRecipientEntryPayload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(25n));
    const missingRecipientEntryState = createState({
        unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: bigIntToBuffer(26n) })]])
    });
    const missingRecipientEntryValidator = new PartialTransferValidator(missingRecipientEntryState, validator.address, config);
    missingRecipientEntryValidator.fee = 1n;
    await missingRecipientEntryValidator.validate(missingRecipientEntryPayload);
    t.pass('recipient entry may be absent for non-self transfer');
});

test('PartialTransferValidator.validate rejects invalid recipient address or public key', async t => {
    const { requester, recipient, validator } = await getWalletSet();
    const payload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(1n));

    const invalidAddressPayload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(1n));
    invalidAddressPayload.tro.to = b4a.alloc(config.addressLength - 1);
    const invalidAddressValidator = new PartialTransferValidator(
        createState({ unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: bigIntToBuffer(10n) })]]) }),
        validator.address,
        config
    );
    invalidAddressValidator.check.validateTransferOperation = () => true;
    invalidAddressValidator.validateSignature = async () => true;
    await expectSharedValidatorError(
        t,
        () => invalidAddressValidator.validate(invalidAddressPayload),
        ResultCode.TRANSFER_RECIPIENT_ADDRESS_INVALID,
        'Invalid recipient address'
    );

    const invalidPublicKeyPayload = await buildTransferPayload(requester, createZeroPublicKeyAddress(), bigIntToBuffer(1n));
    const originalDecodeSafe = tracCryptoApi.address.decodeSafe;
    tracCryptoApi.address.decodeSafe = address => address === createZeroPublicKeyAddress()
        ? b4a.alloc(0)
        : originalDecodeSafe(address);
    t.teardown(() => {
        tracCryptoApi.address.decodeSafe = originalDecodeSafe;
    });

    await expectSharedValidatorError(
        t,
        () => new PartialTransferValidator(
            createState({ unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: bigIntToBuffer(10n) })]]) }),
            validator.address,
            config
        ).validate(invalidPublicKeyPayload),
        ResultCode.TRANSFER_RECIPIENT_PUBLIC_KEY_INVALID,
        'Invalid recipient public key'
    );

    const senderAddress = tracCryptoApi.address.decodeSafe(requester.address);
    t.ok(senderAddress);
    t.ok(payload.tro.to);
});

test('PartialTransferValidator.validate rejects transfer amount and state balance edge cases', async t => {
    const { requester, recipient, validator } = await getWalletSet();
    const payload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(2n));

    const amountValidator = new PartialTransferValidator(
        createState({ unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: bigIntToBuffer(10n) })]]) }),
        validator.address,
        config
    );
    amountValidator.max_amount = 1n;
    await expectSharedValidatorError(
        t,
        () => amountValidator.validate(payload),
        ResultCode.TRANSFER_AMOUNT_TOO_LARGE,
        'exceeds maximum'
    );

    await expectSharedValidatorError(
        t,
        () => new PartialTransferValidator(createState(), validator.address, config).validate(payload),
        ResultCode.TRANSFER_SENDER_NOT_FOUND,
        'Sender account not found'
    );

    const insufficientValidator = new PartialTransferValidator(
        createState({ unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: bigIntToBuffer(2n) })]]) }),
        validator.address,
        config
    );
    insufficientValidator.fee = 1n;
    const insufficientPayload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(2n));
    await expectSharedValidatorError(
        t,
        () => insufficientValidator.validate(insufficientPayload),
        ResultCode.TRANSFER_INSUFFICIENT_BALANCE,
        'Insufficient balance'
    );

    const selfInsufficientValidator = new PartialTransferValidator(
        createState({ unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: bigIntToBuffer(0n) })]]) }),
        validator.address,
        config
    );
    selfInsufficientValidator.fee = 1n;
    const selfInsufficientPayload = await buildTransferPayload(requester, requester.address, bigIntToBuffer(2n));
    await expectSharedValidatorError(
        t,
        () => selfInsufficientValidator.validate(selfInsufficientPayload),
        ResultCode.TRANSFER_INSUFFICIENT_BALANCE,
        'fee'
    );

    const overflowValidator = new PartialTransferValidator(
        createState({
            unsignedEntries: new Map([
                [requester.address, createNodeEntry({ balance: bigIntToBuffer(20n) })],
                [recipient.address, createNodeEntry({ balance: bigIntToBuffer(10n) })]
            ])
        }),
        validator.address,
        config
    );
    overflowValidator.max_amount = 10n;
    overflowValidator.fee = 1n;
    const overflowPayload = await buildTransferPayload(requester, recipient.address, bigIntToBuffer(1n));
    await expectSharedValidatorError(
        t,
        () => overflowValidator.validate(overflowPayload),
        ResultCode.TRANSFER_RECIPIENT_BALANCE_OVERFLOW,
        'recipient balance'
    );
});
