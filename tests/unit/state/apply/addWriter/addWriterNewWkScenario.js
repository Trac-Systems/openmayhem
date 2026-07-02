import { test } from 'brittle';
import b4a from 'b4a';
import { randomBytes } from 'crypto';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import {
	setupAddWriterScenario,
	selectWriterPeer,
	buildAddWriterPayload,
	buildRemoveWriterPayload,
	assertAddWriterSuccessState,
	assertWriterRemovalState
} from './addWriterScenarioHelpers.js';
import {
	toBalance,
	BALANCE_FEE,
	BALANCE_TO_STAKE,
	PERCENT_75
} from '../../../../../src/core/state/utils/balance.js';

export default function addWriterNewWkScenario() {
	test('State.apply addWriter rotates writing keys after removing/re-adding writer role', async t => {
		const context = await setupAddWriterScenario(t);
		const validatorPeer = context.adminBootstrap;
		const writerPeer = selectWriterPeer(context);
		const originalWriterKey = b4a.from(writerPeer.base.local.key);
		const rewardPerOperation = BALANCE_FEE.percentage(PERCENT_75);

		const writerInitialBalance = toBalance(context.addWriterScenario.writerInitialBalance);
		let validatorBalanceTracker = await readNodeBalanceBuffer(validatorPeer);

		const firstAddPayload = await buildAddWriterPayload(context, {
			readerPeer: writerPeer,
			validatorPeer
		});

		await validatorPeer.base.append(firstAddPayload);
		await validatorPeer.base.update();
		await eventFlush();

		await assertAddWriterSuccessState(t, context, {
			readerPeer: writerPeer,
			validatorPeer,
			writerInitialBalance: context.addWriterScenario.writerInitialBalance,
			validatorBalanceBefore: validatorBalanceTracker,
			payload: firstAddPayload,
			writerKeyBuffer: originalWriterKey,
			expectedWriterIndex: 1
		});

		validatorBalanceTracker = advanceBalanceTracker(validatorBalanceTracker, rewardPerOperation);

		const balanceAfterInitialAdd = writerInitialBalance
			.sub(BALANCE_FEE)
			?.sub(BALANCE_TO_STAKE);
		if (!balanceAfterInitialAdd) {
			throw new Error('Failed to derive writer balance after initial add.');
		}

		const removeWriterPayload = await buildRemoveWriterPayload(context, {
			readerPeer: writerPeer,
			validatorPeer,
			writerKeyBuffer: originalWriterKey
		});

		await validatorPeer.base.append(removeWriterPayload);
		await validatorPeer.base.update();
		await eventFlush();

		const writerBalanceAfterRemoval = balanceAfterInitialAdd
			.add(BALANCE_TO_STAKE)
			?.sub(BALANCE_FEE);
		if (!writerBalanceAfterRemoval) {
			throw new Error('Failed to derive writer balance after removal.');
		}

		await assertWriterRemovalState(t, context, {
			readerPeer: writerPeer,
			validatorPeer,
			writerKeyBuffer: originalWriterKey,
			expectedBalanceBuffer: b4a.from(writerBalanceAfterRemoval.value),
			payload: removeWriterPayload
		});

		validatorBalanceTracker = advanceBalanceTracker(validatorBalanceTracker, rewardPerOperation);

		const rotatedWriterKey = randomBytes(32);
		const writerBalanceBeforeReAdd = b4a.from(writerBalanceAfterRemoval.value);

		const secondAddPayload = await buildAddWriterPayload(context, {
			readerPeer: writerPeer,
			validatorPeer,
			writerKeyBuffer: rotatedWriterKey
		});

		await validatorPeer.base.append(secondAddPayload);
		await validatorPeer.base.update();
		await eventFlush();

		await assertAddWriterSuccessState(t, context, {
			readerPeer: writerPeer,
			validatorPeer,
			writerInitialBalance: writerBalanceBeforeReAdd,
			validatorBalanceBefore: validatorBalanceTracker,
			payload: secondAddPayload,
			expectedWriterIndex: 2,
			writerKeyBuffer: rotatedWriterKey
		});

		validatorBalanceTracker = advanceBalanceTracker(validatorBalanceTracker, rewardPerOperation);

		const finalAdminBalance = await readNodeBalanceBuffer(validatorPeer);
		t.ok(
			b4a.equals(finalAdminBalance, validatorBalanceTracker),
			'validator accumulated rewards from two addWriter operations and one removeWriter'
		);

		const writerBalanceAfterReAdd = writerBalanceAfterRemoval
			.sub(BALANCE_FEE)
			?.sub(BALANCE_TO_STAKE);
		if (!writerBalanceAfterReAdd) {
			throw new Error('Failed to compute writer balance after re-add.');
		}

		const writerEntry = await writerPeer.base.view.get(writerPeer.wallet.address);
		const decodedWriter = nodeEntryUtils.decode(writerEntry.value);
		const expectedWriterBalance = writerBalanceAfterReAdd.value;
		t.ok(
			b4a.equals(decodedWriter.balance, expectedWriterBalance),
			'writer ends with balance reduced by three fees and new stake'
		);
		t.ok(
			b4a.equals(decodedWriter.wk, rotatedWriterKey),
			'writer entry reflects the rotated writing key'
		);
	});
}

function advanceBalanceTracker(currentBuffer, incrementBalance) {
	const current = toBalance(currentBuffer);
	const next = current.add(incrementBalance);
	return b4a.from(next.value);
}

async function readNodeBalanceBuffer(peer) {
	const entry = await peer.base.view.get(peer.wallet.address);
	const decoded = nodeEntryUtils.decode(entry.value);
	return b4a.from(decoded.balance);
}
