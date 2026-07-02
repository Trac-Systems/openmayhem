import { test } from 'brittle';
import b4a from 'b4a';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import {
	setupRemoveWriterScenario,
	selectWriterPeer,
	buildRemoveWriterPayload,
	assertRemoveWriterSuccessState
} from './removeWriterScenarioHelpers.js';
import {
	buildAddWriterPayload,
	assertAddWriterSuccessState
} from '../addWriter/addWriterScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import lengthEntryUtils from '../../../../../src/core/state/utils/lengthEntry.js';
import { EntryType } from '../../../../../src/utils/constants.js';

export default function removeWriterAndAddWriterAgainScenario() {
	test('State.apply removeWriter followed by addWriter re-promotes the same node', async t => {
		const context = await setupRemoveWriterScenario(t);
		const validatorPeer = context.adminBootstrap;
		const writerPeer = selectWriterPeer(context);
		const writerAddress = writerPeer.wallet.address;

		const writerEntryBeforeRemoval = await validatorPeer.base.view.get(writerAddress);
		t.ok(writerEntryBeforeRemoval, 'writer entry exists before removeWriter');
		const validatorEntryBeforeRemoval = await validatorPeer.base.view.get(
			validatorPeer.wallet.address
		);
		t.ok(validatorEntryBeforeRemoval, 'validator entry exists before removeWriter');

		const removeWriterPayload = await buildRemoveWriterPayload(context, {
			readerPeer: writerPeer,
			validatorPeer
		});

		await validatorPeer.base.append(removeWriterPayload);
		await validatorPeer.base.update();
		await eventFlush();

		await assertRemoveWriterSuccessState(t, context, {
			writerPeer,
			validatorPeer,
			writerEntryBefore: writerEntryBeforeRemoval,
			validatorEntryBefore: validatorEntryBeforeRemoval,
			payload: removeWriterPayload
		});

		const validatorEntryBeforeReAdd = await validatorPeer.base.view.get(
			validatorPeer.wallet.address
		);
		t.ok(validatorEntryBeforeReAdd, 'validator entry exists before addWriter re-promotion');
		const decodedValidatorBeforeReAdd = nodeEntryUtils.decode(validatorEntryBeforeReAdd.value);
		t.ok(decodedValidatorBeforeReAdd, 'validator entry decodes before addWriter re-promotion');

		const writerEntryAfterRemoval = await validatorPeer.base.view.get(writerAddress);
		t.ok(writerEntryAfterRemoval, 'writer entry exists after removeWriter');
		const decodedWriterAfterRemoval = nodeEntryUtils.decode(writerEntryAfterRemoval.value);
		t.ok(decodedWriterAfterRemoval, 'writer entry decodes after removeWriter');

		const writerBalanceBeforeReAdd = b4a.from(decodedWriterAfterRemoval.balance);
		const validatorBalanceBeforeReAdd = b4a.from(decodedValidatorBeforeReAdd.balance);
		const writersLengthEntry = await validatorPeer.base.view.get(EntryType.WRITERS_LENGTH);
		const nextWriterIndex = writersLengthEntry
			? lengthEntryUtils.decodeBE(writersLengthEntry.value)
			: 0;

		const reAddPayload = await buildAddWriterPayload(context, {
			readerPeer: writerPeer,
			validatorPeer
		});

		await validatorPeer.base.append(reAddPayload);
		await validatorPeer.base.update();
		await eventFlush();

		await assertAddWriterSuccessState(t, context, {
			readerPeer: writerPeer,
			validatorPeer,
			writerInitialBalance: writerBalanceBeforeReAdd,
			validatorBalanceBefore: validatorBalanceBeforeReAdd,
			payload: reAddPayload,
			writerKeyBuffer: writerPeer.base.local.key,
			expectedWriterIndex: nextWriterIndex
		});
	});
}
