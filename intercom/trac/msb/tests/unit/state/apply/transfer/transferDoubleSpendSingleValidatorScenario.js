import { test } from 'brittle';
import {
	setupTransferScenario,
	buildTransferPayload,
	assertTransferSuccessState,
	snapshotTransferEntries,
	DEFAULT_TRANSFER_AMOUNT,
	DEFAULT_INITIAL_BALANCE
} from './transferScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { toBalance } from '../../../../../src/core/state/utils/balance.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';

export default function transferDoubleSpendSingleValidatorScenario() {
	test('State.apply transfer prevents double spend via same validator (separate appends, distinct tx hashes)', async t => {
		/* Sender has only amount+fee; same validator processes two distinct transfers (different nonces/hashes) to two recipients. First applies, second skipped. */
		const amountBalance = toBalance(DEFAULT_TRANSFER_AMOUNT);
		const feeBalance = toBalance(transactionUtils.FEE);
		const initialSenderBalance = amountBalance && feeBalance ? amountBalance.add(feeBalance)?.value : DEFAULT_INITIAL_BALANCE;
		t.ok(initialSenderBalance, 'initial sender balance computed');

		const context = await setupTransferScenario(t, {
			nodes: 5,
			recipientHasEntry: false,
			senderInitialBalance: initialSenderBalance
		});

		const nonAdmin = context.peers.slice(1);
		const { senderPeer, recipientPeer, validatorPeer } = context.transferScenario;
		const recipientB = nonAdmin[3];
		t.ok(recipientB, 'second recipient available');

		const snapshots = await snapshotTransferEntries(context, { senderPeer, recipientPeer, validatorPeer });
		const payloadA = await buildTransferPayload(context, { recipientPeer });
		const payloadB = await buildTransferPayload(context, { recipientPeer: recipientB });

		// First transfer
		await validatorPeer.base.append(payloadA);
		await validatorPeer.base.update();
		await eventFlush();

		// Second transfer (should be ignored due to insufficient balance)
		await validatorPeer.base.append(payloadB);
		await validatorPeer.base.update();
		await eventFlush();

		await assertTransferSuccessState(t, context, {
			payload: payloadA,
			senderEntryBefore: snapshots.senderEntry,
			recipientEntryBefore: snapshots.recipientEntry,
			validatorEntryBefore: snapshots.validatorEntry
		});

		// Second recipient should remain untouched (no entry if it did not exist).
		const recipientBAfter = await validatorPeer.base.view.get(recipientB.wallet.address);
		t.is(recipientBAfter, null, 'second recipient not credited after insufficient funds');

		// Second tx hash should not be recorded.
		const decodedB = safeDecodeApplyOperation(payloadB);
		const txHashB = decodedB?.tro?.tx?.toString('hex') ?? '';
		if (txHashB) {
			const txEntryB = await validatorPeer.base.view.get(txHashB);
			t.is(txEntryB, null, 'second tx hash not recorded after insufficient funds');
		}
	});
}
