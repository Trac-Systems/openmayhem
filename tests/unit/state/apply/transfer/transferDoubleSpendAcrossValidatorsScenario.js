import b4a from 'b4a';
import { test } from 'brittle';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import { safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { deriveIndexerSequenceState, eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupTransferScenario,
	DEFAULT_TRANSFER_AMOUNT,
	DEFAULT_INITIAL_BALANCE
} from './transferScenarioHelpers.js';
import { initializeBalances, whitelistAddress } from '../common/commonScenarioHelper.js';
import { promotePeerToWriter } from '../addWriter/addWriterScenarioHelpers.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';
import { toBalance } from '../../../../../src/core/state/utils/balance.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { config } from '../../../../helpers/config.js';

export default function transferDoubleSpendAcrossValidatorsScenario() {
	test('State.apply transfer prevents double spend across validators (distinct tx hashes)', async t => {
		/* Sender has only amount+fee; sends two distinct transfers (different nonces/hashes) to two recipients via two validators. First succeeds, second is skipped (insufficient balance). */
		const amountBalance = toBalance(DEFAULT_TRANSFER_AMOUNT);
		const feeBalance = toBalance(transactionUtils.FEE);
		const initialSenderBalance = amountBalance && feeBalance ? amountBalance.add(feeBalance)?.value : DEFAULT_INITIAL_BALANCE;
		t.ok(initialSenderBalance, 'initial sender balance computed');

		const context = await setupTransferScenario(t, {
			nodes: 6,
			recipientHasEntry: false,
			senderInitialBalance: initialSenderBalance
		});

		const nonAdmin = context.peers.slice(1);
		const primaryValidator = context.transferScenario.validatorPeer;
		const senderPeer = context.transferScenario.senderPeer;
		const recipientA = context.transferScenario.recipientPeer;
		const secondaryValidator = nonAdmin[3];
		const recipientB = nonAdmin[4];
		t.ok(recipientB, 'second recipient available');

		// Prepare secondary validator as writer with funds.
		await initializeBalances(context, [[secondaryValidator.wallet.address, DEFAULT_INITIAL_BALANCE]]);
		await whitelistAddress(context, secondaryValidator.wallet.address);
		await promotePeerToWriter(t, context, {
			readerPeer: secondaryValidator,
			validatorPeer: context.adminBootstrap,
			expectedWriterIndex: 2
		});
		await context.sync();

		const txValidityA = await deriveIndexerSequenceState(primaryValidator.base);
		const txValidityB = await deriveIndexerSequenceState(secondaryValidator.base);

        const partialA = await applyStateMessageFactory(senderPeer.wallet, config)
            .buildPartialTransferOperationMessage(
                senderPeer.wallet.address,
                recipientA.wallet.address,
                b4a.toString(DEFAULT_TRANSFER_AMOUNT, 'hex'),
                b4a.toString(txValidityA, 'hex'),
                'json'
            );
        const partialB = await applyStateMessageFactory(senderPeer.wallet, config)
            .buildPartialTransferOperationMessage(
                senderPeer.wallet.address,
                recipientB.wallet.address,
                b4a.toString(DEFAULT_TRANSFER_AMOUNT, 'hex'),
                b4a.toString(txValidityB, 'hex'),
                'json'
            );

        const payloadA = safeEncodeApplyOperation(
            await applyStateMessageFactory(primaryValidator.wallet, config)
                .buildCompleteTransferOperationMessage(
                    partialA.address,
                    b4a.from(partialA.tro.tx, 'hex'),
                    b4a.from(partialA.tro.txv, 'hex'),
                    b4a.from(partialA.tro.in, 'hex'),
                    partialA.tro.to,
                    b4a.from(partialA.tro.am, 'hex'),
                    b4a.from(partialA.tro.is, 'hex')
                )
        );

        const payloadB = safeEncodeApplyOperation(
            await applyStateMessageFactory(secondaryValidator.wallet, config)
                .buildCompleteTransferOperationMessage(
                    partialB.address,
                    b4a.from(partialB.tro.tx, 'hex'),
                    b4a.from(partialB.tro.txv, 'hex'),
                    b4a.from(partialB.tro.in, 'hex'),
                    partialB.tro.to,
                    b4a.from(partialB.tro.am, 'hex'),
                    b4a.from(partialB.tro.is, 'hex')
                )
        );

		// Apply first transfer successfully via primary validator.
		await primaryValidator.base.append(payloadA);
		await primaryValidator.base.update();
		await eventFlush();
		await context.sync();

		const senderAfterFirst = await primaryValidator.base.view.get(senderPeer.wallet.address);
		const recipientAfterFirst = await primaryValidator.base.view.get(recipientA.wallet.address);
		const validatorAfterFirst = await primaryValidator.base.view.get(primaryValidator.wallet.address);

		t.ok(senderAfterFirst?.value, 'sender entry exists after first transfer');
		t.ok(recipientAfterFirst?.value, 'first recipient entry exists after first transfer');
		t.ok(validatorAfterFirst?.value, 'primary validator entry exists after first transfer');

		const txHashA = partialA.tro.tx.toString('hex');
		const txEntryA = await primaryValidator.base.view.get(txHashA);
		t.ok(txEntryA, 'first tx hash recorded');

		// Attempt second transfer via secondary validator (should be ignored: insufficient balance).
		await secondaryValidator.base.append(payloadB);
		await secondaryValidator.base.update();
		await eventFlush();
		await context.sync();

		const senderAfterSecond = await secondaryValidator.base.view.get(senderPeer.wallet.address);
		const recipientBAfter = await secondaryValidator.base.view.get(recipientB.wallet.address);
		const validatorBAfter = await secondaryValidator.base.view.get(secondaryValidator.wallet.address);

		// Sender balance unchanged vs after first.
		const senderBalanceFirst = nodeEntryUtils.decode(senderAfterFirst?.value ?? Buffer.alloc(0))?.balance;
		const senderBalanceSecond = nodeEntryUtils.decode(senderAfterSecond?.value ?? Buffer.alloc(0))?.balance;
		t.ok(senderBalanceFirst && senderBalanceSecond && b4a.equals(senderBalanceFirst, senderBalanceSecond), 'sender balance unchanged after second attempt');

		// Recipient B did not receive funds.
		t.ok(!recipientBAfter?.value, 'second recipient entry absent after ignored transfer');

		// Secondary validator not rewarded.
		const validatorBalanceB = nodeEntryUtils.decode(validatorBAfter?.value ?? Buffer.alloc(0))?.balance;
		const validatorInitialB = await primaryValidator.base.view.get(secondaryValidator.wallet.address);
		const validatorBalanceInitialB = nodeEntryUtils.decode(validatorInitialB?.value ?? Buffer.alloc(0))?.balance;
		if (validatorBalanceB && validatorBalanceInitialB) {
			t.ok(b4a.equals(validatorBalanceB, validatorBalanceInitialB), 'secondary validator balance unchanged');
		}

		const txHashB = partialB.tro.tx.toString('hex');
		const txEntryB = await secondaryValidator.base.view.get(txHashB);
		t.is(txEntryB, null, 'second tx hash not recorded after insufficient balance');

		t.ok(nodeEntryUtils.decode(recipientAfterFirst?.value ?? Buffer.alloc(0)), 'recipient A decodes');
	});
}
