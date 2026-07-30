import test from 'brittle';
import b4a from 'b4a';

import PartialTransactionValidator from '../../../../../src/core/network/protocols/shared/validators/PartialTransactionValidator.js';
import { ResultCode } from '../../../../../src/utils/constants.js';
import { $TNK } from '../../../../../src/core/state/utils/balance.js';
import { config } from '../../../../helpers/config.js';
import {
    createNodeEntry,
    createState,
    getWalletSet,
    buildBootstrapDeploymentPayload,
    buildTransactionPayload,
    createBootstrapTransactionRecord,
    createDeploymentRegistrationEntry,
    expectSharedValidatorError
} from '../../utils/sharedValidatorTestUtils.js';

test('PartialTransactionValidator.validate accepts deployed external bootstrap references', async t => {
    const { requester, validator } = await getWalletSet();
    const deploymentPayload = await buildBootstrapDeploymentPayload(requester);
    const deploymentEntry = createDeploymentRegistrationEntry(deploymentPayload, requester.address);
    const txRecord = createBootstrapTransactionRecord(deploymentPayload);
    const payload = await buildTransactionPayload(requester, undefined, { externalBootstrap: deploymentPayload.bdo.bs });
    const txHex = deploymentPayload.bdo.tx.toString('hex');

    const state = createState({
        signedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n) })]]),
        unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n) })]]),
        txEntries: new Map([[txHex, txRecord]]),
        registeredBootstrapEntries: new Map([[payload.txo.bs.toString('hex'), deploymentEntry]])
    });

    await new PartialTransactionValidator(state, validator.address, config).validate(payload);
    t.pass();
});

test('PartialTransactionValidator.validate rejects bootstrap mismatches and missing deployment state', async t => {
    const { requester, validator } = await getWalletSet();
    const payload = await buildTransactionPayload(requester);
    const mismatchedMsbPayload = await buildTransactionPayload(requester, undefined, { msbBootstrap: b4a.alloc(32, 0xab) });

    await expectSharedValidatorError(
        t,
        () => new PartialTransactionValidator(
            createState({
                signedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n) })]]),
                unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n) })]])
            }),
            validator.address,
            config
        ).validate(mismatchedMsbPayload),
        ResultCode.MSB_BOOTSTRAP_MISMATCH,
        'Declared MSB bootstrap'
    );

    await expectSharedValidatorError(
        t,
        () => new PartialTransactionValidator(
            createState({
                signedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n) })]]),
                unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n) })]])
            }),
            validator.address,
            config
        ).validate(payload),
        ResultCode.EXTERNAL_BOOTSTRAP_NOT_DEPLOYED,
        'not registered as deployment entry'
    );
});

test('PartialTransactionValidator.validate rejects missing or mismatched external bootstrap transaction records', async t => {
    const { requester, validator, alternate } = await getWalletSet();
    const deploymentPayload = await buildBootstrapDeploymentPayload(requester);
    const payload = await buildTransactionPayload(requester, undefined, { externalBootstrap: deploymentPayload.bdo.bs });
    const deploymentEntry = createDeploymentRegistrationEntry(deploymentPayload, requester.address);

    await expectSharedValidatorError(
        t,
        () => new PartialTransactionValidator(
            createState({
                signedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n) })]]),
                unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n) })]]),
                registeredBootstrapEntries: new Map([[payload.txo.bs.toString('hex'), deploymentEntry]])
            }),
            validator.address,
            config
        ).validate(payload),
        ResultCode.EXTERNAL_BOOTSTRAP_TX_MISSING,
        'not registered as usual tx'
    );

    const mismatchedDeploymentPayload = await buildBootstrapDeploymentPayload(alternate, undefined, b4a.alloc(32, 0xcc));
    const mismatchedTxRecord = createBootstrapTransactionRecord(mismatchedDeploymentPayload);
    await expectSharedValidatorError(
        t,
        () => new PartialTransactionValidator(
            createState({
                signedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n) })]]),
                unsignedEntries: new Map([[requester.address, createNodeEntry({ balance: $TNK(100n) })]]),
                txEntries: new Map([[deploymentPayload.bdo.tx.toString('hex'), mismatchedTxRecord]]),
                registeredBootstrapEntries: new Map([[payload.txo.bs.toString('hex'), deploymentEntry]])
            }),
            validator.address,
            config
        ).validate(payload),
        ResultCode.EXTERNAL_BOOTSTRAP_MISMATCH,
        'does not match the one in the transaction payload'
    );
});
