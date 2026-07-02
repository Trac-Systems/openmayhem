import { test } from 'brittle';
import {
	setupTransferScenario,
	buildTransferPayload,
	assertTransferSuccessState,
	snapshotTransferEntries,
	DEFAULT_TRANSFER_AMOUNT
} from './transferScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

export default function transferValidatorRecipientAmountScenario() {
	test('State.apply transfer credits validator recipient with amount and fee (amount > 0)', async t => {
		/* Recipient is the validator, sender pays amount+fee, validator receives amount plus 75% fee. */
		const context = await setupTransferScenario(t, { recipientHasEntry: true });
		const { senderPeer, validatorPeer } = context.transferScenario;

		const snapshots = await snapshotTransferEntries(context, {
			senderPeer,
			recipientPeer: validatorPeer,
			validatorPeer
		});
		const payload = await buildTransferPayload(context, {
			recipientPeer: validatorPeer,
			amount: DEFAULT_TRANSFER_AMOUNT
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
