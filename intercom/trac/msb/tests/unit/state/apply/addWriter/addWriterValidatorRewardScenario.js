import { test } from 'brittle';
import b4a from 'b4a';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import {
	setupAddWriterScenario,
	selectWriterPeer,
	buildAddWriterPayload,
	assertAddWriterSuccessState,
	defaultWriterFunding
} from './addWriterScenarioHelpers.js';
import { initializeBalances, whitelistAddress } from '../common/commonScenarioHelper.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import { config } from '../../../../helpers/config.js';

export default function addWriterValidatorRewardScenario() {
	test(
		'State.apply addWriter rewards validator writers that process peer promotions',
		async t => {
			const context = await setupAddWriterScenario(t, { nodes: 3 });
			const adminPeer = context.adminBootstrap;
			const firstWriterPeer = selectWriterPeer(context, 0);
			const secondReaderPeer = selectWriterPeer(context, 1);
			if (!secondReaderPeer) {
				t.fail('Validator reward scenario requires a second reader peer.');
				return;
			}

			await fundAndWhitelistPeer(context, secondReaderPeer);

			const adminBalanceBefore = await readNodeBalanceBuffer(adminPeer);
			const firstAddPayload = await buildAddWriterPayload(context, {
				readerPeer: firstWriterPeer,
				validatorPeer: adminPeer
			});

			await appendPayload(adminPeer.base, firstAddPayload);

			await assertAddWriterSuccessState(t, context, {
				readerPeer: firstWriterPeer,
				validatorPeer: adminPeer,
				writerInitialBalance: context.addWriterScenario?.writerInitialBalance,
				validatorBalanceBefore: adminBalanceBefore,
				payload: firstAddPayload,
				expectedWriterIndex: 1
			});

			const writerBalanceBeforeReward = await readNodeBalanceBuffer(firstWriterPeer);
			const secondAddPayload = await buildAddWriterPayload(context, {
				readerPeer: secondReaderPeer,
				validatorPeer: firstWriterPeer
			});

			assertPayloadValidator(t, secondAddPayload, firstWriterPeer.wallet.address);

			await appendPayload(firstWriterPeer.base, secondAddPayload);

			await assertAddWriterSuccessState(t, context, {
				readerPeer: secondReaderPeer,
				validatorPeer: firstWriterPeer,
				writerInitialBalance:
					context.addWriterScenario?.writerInitialBalance ?? defaultWriterFunding,
				validatorBalanceBefore: writerBalanceBeforeReward,
				payload: secondAddPayload,
				expectedWriterIndex: 2
			});
		}
	);
}

async function fundAndWhitelistPeer(context, peer) {
	const funding = context.addWriterScenario?.writerInitialBalance ?? defaultWriterFunding;
	await initializeBalances(context, [[peer.wallet.address, funding]]);
	await whitelistAddress(context, peer.wallet.address);
}

async function appendPayload(nodeBase, payload) {
	await nodeBase.append(payload);
	await nodeBase.update();
	await eventFlush();
}

async function readNodeBalanceBuffer(peer) {
	const entry = await peer.base.view.get(peer.wallet.address);
	if (!entry?.value) {
		throw new Error('Unable to read node balance for validator reward scenario.');
	}
	const decoded = nodeEntryUtils.decode(entry.value);
	if (!decoded?.balance) {
		throw new Error('Validator entry decode failed while tracking rewards.');
	}
	return b4a.from(decoded.balance);
}

function assertPayloadValidator(t, payload, validatorAddress) {
	const decoded = safeDecodeApplyOperation(payload);
	t.ok(decoded, 'validator reward payload decodes');
	const validatorBuffer = decoded?.rao?.va;
	t.ok(validatorBuffer, 'payload carries validator address');
	const expected = addressUtils.addressToBuffer(validatorAddress, config.addressPrefix);
	t.ok(
		b4a.equals(validatorBuffer, expected),
		'payload validator address matches processing writer'
	);
}
