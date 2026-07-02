import { test } from 'brittle';
import {
	setupTransferScenario,
	buildTransferPayload,
	assertTransferSuccessState,
	snapshotTransferEntries,
	ZERO_TRANSFER_AMOUNT
} from './transferScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

export default function transferSelfTransferZeroAmountScenario() {
	test('State.apply transfer charges fee on self-transfer (amount = 0)', async t => {
		/* Sender transfers zero to self, fee is deducted, validator earns 75% fee. */
		const context = await setupTransferScenario(t, { recipientHasEntry: true });
		const { senderPeer, validatorPeer } = context.transferScenario;

		const snapshots = await snapshotTransferEntries(context, {
			senderPeer,
			recipientPeer: senderPeer,
			validatorPeer
		});
		const payload = await buildTransferPayload(context, {
			recipientPeer: senderPeer,
			amount: ZERO_TRANSFER_AMOUNT
		});

		await validatorPeer.base.append(payload);
		await validatorPeer.base.update();
		await eventFlush();

		await assertTransferSuccessState(t, context, {
			payload,
			senderEntryBefore: snapshots.senderEntry,
			recipientEntryBefore: snapshots.recipientEntry,
			validatorEntryBefore: snapshots.validatorEntry
		});
	});
}
