import { test } from 'brittle';
import {
	setupTransferScenario,
	buildTransferPayload,
	assertTransferSuccessState,
	snapshotTransferEntries,
	DEFAULT_TRANSFER_AMOUNT
} from './transferScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

export default function transferNewRecipientAmountScenario() {
	test('State.apply transfer creates recipient entry when missing (amount > 0)', async t => {
		/* Missing recipient gets new reader entry with amount, sender pays amount+fee, validator earns 75% fee. */
		const context = await setupTransferScenario(t, { recipientHasEntry: false });
		const { senderPeer, recipientPeer, validatorPeer } = context.transferScenario;

		const snapshots = await snapshotTransferEntries(context, { senderPeer, recipientPeer, validatorPeer });
		const payload = await buildTransferPayload(context, { amount: DEFAULT_TRANSFER_AMOUNT });

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
