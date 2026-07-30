import test from 'brittle';
import b4a from 'b4a';

import PartialBootstrapDeploymentValidator from '../../../../../src/core/network/protocols/shared/validators/PartialBootstrapDeploymentValidator.js';
import { ResultCode } from '../../../../../src/utils/constants.js';
import { bufferToBigInt } from '../../../../../src/utils/amountSerialization.js';
import { FEE } from '../../../../../src/core/state/utils/transaction.js';
import { $TNK } from '../../../../../src/core/state/utils/balance.js';
import { bigIntToBuffer } from '../../../../../src/utils/buffer.js';
import { config } from '../../../../helpers/config.js';
import {
    createNodeEntry,
    createState,
    getWalletSet,
    buildBootstrapDeploymentPayload,
    expectSharedValidatorError
} from '../../utils/sharedValidatorTestUtils.js';

const FEE_BIGINT = bufferToBigInt(FEE);

test('PartialBootstrapDeploymentValidator.validate accepts unique bootstrap payload with exact fee balance', async t => {
    const { requester, validator } = await getWalletSet();
    const payload = await buildBootstrapDeploymentPayload(requester);
    const state = createState({
        signedEntries: new Map([[requester.address, createNodeEntry({ balance: bigIntToBuffer(FEE_BIGINT) })]]),
        unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: bigIntToBuffer(FEE_BIGINT) })]])
    });

    await new PartialBootstrapDeploymentValidator(state, validator.address, config).validate(payload);
    t.pass();
});

test('PartialBootstrapDeploymentValidator.validate rejects duplicated and invalid bootstrap registrations', async t => {
    const { requester, validator } = await getWalletSet();
    const payload = await buildBootstrapDeploymentPayload(requester);

    await expectSharedValidatorError(
        t,
        () => new PartialBootstrapDeploymentValidator(
            createState({
                signedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n) })]]),
                unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n) })]]),
                registeredBootstrapEntriesUnsigned: new Map([[payload.bdo.bs.toString('hex'), b4a.from('exists')]])
            }),
            validator.address,
            config
        ).validate(payload),
        ResultCode.BOOTSTRAP_ALREADY_EXISTS,
        'already exists'
    );

    const sameBootstrapPayload = await buildBootstrapDeploymentPayload(requester, undefined, config.bootstrap);
    await expectSharedValidatorError(
        t,
        () => new PartialBootstrapDeploymentValidator(
            createState({
                signedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n) })]]),
                unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n) })]])
            }),
            validator.address,
            config
        ).validate(sameBootstrapPayload),
        ResultCode.EXTERNAL_BOOTSTRAP_EQUALS_MSB_BOOTSTRAP,
        'same as MSB bootstrap'
    );
});
