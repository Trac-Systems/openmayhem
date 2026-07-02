import { test } from 'brittle';
import {
	setupTransferScenario,
	buildTransferPayload,
	assertTransferSuccessState,
	snapshotTransferEntries,
	ZERO_TRANSFER_AMOUNT
} from './transferScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

export default function transferValidatorRecipientZeroAmountScenario() {
	test('State.apply transfer rewards validator recipient with fee only (amount = 0)', async t => {
		/* Recipient is validator, sender pays fee, validator/recipient receives 75% fee. */
		const context = await setupTransferScenario(t, { recipientHasEntry: true });
		const { senderPeer, validatorPeer } = context.transferScenario;

		const snapshots = await snapshotTransferEntries(context, {
			senderPeer,
			recipientPeer: validatorPeer,
			validatorPeer
		});
		const payload = await buildTransferPayload(context, {
			recipientPeer: validatorPeer,
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
