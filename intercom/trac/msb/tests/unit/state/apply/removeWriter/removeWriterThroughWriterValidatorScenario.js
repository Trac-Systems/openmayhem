import { test } from 'brittle';
import b4a from 'b4a';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import {
	setupAddWriterScenario,
	selectWriterPeer,
	buildAddWriterPayload,
	assertAddWriterSuccessState,
	defaultWriterFunding
} from '../addWriter/addWriterScenarioHelpers.js';
import {
	buildRemoveWriterPayload,
	assertRemoveWriterSuccessState
} from './removeWriterScenarioHelpers.js';
import { initializeBalances, whitelistAddress } from '../common/commonScenarioHelper.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';

export default function removeWriterThroughWriterValidatorScenario() {
	test(
		'State.apply removeWriter rewards writer validators processing peer removals',
		async t => {
			const context = await setupAddWriterScenario(t, { nodes: 3 });
			const adminPeer = context.adminBootstrap;
			const validatorWriterPeer = selectWriterPeer(context, 0);
			const targetWriterPeer = selectWriterPeer(context, 1);
			if (!targetWriterPeer) {
				t.fail('Writer validator scenario requires a second reader peer.');
				return;
			}

			await fundAndWhitelistPeer(context, targetWriterPeer);

			const adminBalanceBeforeFirstAdd = await readNodeBalanceBuffer(adminPeer);
			const firstAddPayload = await buildAddWriterPayload(context, {
				readerPeer: validatorWriterPeer,
				validatorPeer: adminPeer
			});

			await appendPayload(adminPeer.base, firstAddPayload);

			await assertAddWriterSuccessState(t, context, {
				readerPeer: validatorWriterPeer,
				validatorPeer: adminPeer,
				writerInitialBalance: context.addWriterScenario?.writerInitialBalance,
				validatorBalanceBefore: adminBalanceBeforeFirstAdd,
				payload: firstAddPayload,
				expectedWriterIndex: 1
			});

			const adminBalanceBeforeSecondAdd = await readNodeBalanceBuffer(adminPeer);
			const targetWriterInitialBalance = await readNodeBalanceBuffer(targetWriterPeer);
			const secondAddPayload = await buildAddWriterPayload(context, {
				readerPeer: targetWriterPeer,
				validatorPeer: adminPeer
			});

			await appendPayload(adminPeer.base, secondAddPayload);

			await assertAddWriterSuccessState(t, context, {
				readerPeer: targetWriterPeer,
				validatorPeer: adminPeer,
				writerInitialBalance:
					targetWriterInitialBalance ?? context.addWriterScenario?.writerInitialBalance,
				validatorBalanceBefore: adminBalanceBeforeSecondAdd,
				payload: secondAddPayload,
				expectedWriterIndex: 2
			});

			const writerEntryBeforeRemoval = await adminPeer.base.view.get(targetWriterPeer.wallet.address);
			t.ok(writerEntryBeforeRemoval, 'target writer entry exists before removeWriter');
			const validatorEntryBeforeRemoval = await validatorWriterPeer.base.view.get(
				validatorWriterPeer.wallet.address
			);
			t.ok(validatorEntryBeforeRemoval, 'validator writer entry exists before removeWriter');

			const removeWriterPayload = await buildRemoveWriterPayload(context, {
				readerPeer: targetWriterPeer,
				validatorPeer: validatorWriterPeer,
				writerKeyBuffer: targetWriterPeer.base.local.key
			});

			await appendPayload(validatorWriterPeer.base, removeWriterPayload);

			await assertRemoveWriterSuccessState(t, context, {
				writerPeer: targetWriterPeer,
				validatorPeer: validatorWriterPeer,
				writerEntryBefore: writerEntryBeforeRemoval,
				validatorEntryBefore: validatorEntryBeforeRemoval,
				payload: removeWriterPayload
			});
		}
	);
}

async function fundAndWhitelistPeer(context, peer) {
	const funding = context.addWriterScenario?.writerInitialBalance ?? defaultWriterFunding;
	await initializeBalances(context, [[peer.wallet.address, funding]]);
	await whitelistAddress(context, peer.wallet.address);
}

async function appendPayload(base, payload) {
	await base.append(payload);
	await base.update();
	await eventFlush();
}

async function readNodeBalanceBuffer(peer) {
	const entry = await peer.base.view.get(peer.wallet.address);
	if (!entry?.value) return null;
	const decoded = nodeEntryUtils.decode(entry.value);
	if (!decoded?.balance) return null;
	return b4a.from(decoded.balance);
}
