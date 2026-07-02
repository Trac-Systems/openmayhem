import { test } from 'brittle';
import b4a from 'b4a';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import nodeEntryUtils, { ZERO_LICENSE } from '../../../../../src/core/state/utils/nodeEntry.js';
import { toTerm } from '../../../../../src/core/state/utils/balance.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import setupAppendWhitelistScenario, {
	buildAppendWhitelistPayload,
	selectReaderPeer,
	assertReaderWhitelisted
} from './appendWhitelistScenarioHelpers.js';
import { buildBalanceInitializationPayload } from '../balanceInitialization/balanceInitializationScenarioHelpers.js';

export default function appendWhitelistExistingReaderHappyPathScenario() {
	test('State.apply appendWhitelist upgrades initialized readers without altering their balances', async t => {
		const context = await setupAppendWhitelistScenario(t);
		const adminNode = context.adminBootstrap;
		const readerPeer = selectReaderPeer(context);
		const initialReaderBalance = toTerm(75n);

		const adminNodeEntryBefore = await adminNode.base.view.get(adminNode.wallet.address);
		t.ok(adminNodeEntryBefore, 'admin node entry exists');
		const decodedAdminBefore = nodeEntryUtils.decode(adminNodeEntryBefore.value);
		t.ok(decodedAdminBefore, 'admin node entry decodes');
		const adminBalanceSnapshot = decodedAdminBefore.balance && b4a.from(decodedAdminBefore.balance);

		const balanceInitializationPayload = await buildBalanceInitializationPayload(
			context,
			readerPeer.wallet.address,
			initialReaderBalance
		);
		await adminNode.base.append(balanceInitializationPayload);
		await adminNode.base.update();
		await eventFlush();

		const readerEntryAfterInitialization = await adminNode.base.view.get(readerPeer.wallet.address);
		t.ok(readerEntryAfterInitialization, 'reader entry created via balance initialization');
		const decodedReaderBeforeWhitelist = nodeEntryUtils.decode(readerEntryAfterInitialization.value);
		t.ok(decodedReaderBeforeWhitelist, 'reader entry decodes before whitelist');
		t.ok(
			b4a.equals(decodedReaderBeforeWhitelist.balance, initialReaderBalance),
			'reader balance stored during initialization'
		);
		t.is(decodedReaderBeforeWhitelist.isWhitelisted, false, 'reader not whitelisted yet');
		t.ok(
			b4a.equals(decodedReaderBeforeWhitelist.license, ZERO_LICENSE),
			'reader license absent before whitelist'
		);

		const appendWhitelistPayload = await buildAppendWhitelistPayload(
			context,
			readerPeer.wallet.address
		);
		const decodedPayload = safeDecodeApplyOperation(appendWhitelistPayload);
		t.ok(decodedPayload, 'append whitelist payload decodes');
		const whitelistTxHash = decodedPayload?.aco?.tx?.toString('hex');
		t.ok(whitelistTxHash, 'whitelist tx hash extracted');

		await adminNode.base.append(appendWhitelistPayload);
		await adminNode.base.update();
		await eventFlush();

		const { decodedEntry: decodedReaderAfterWhitelist } = await assertReaderWhitelisted(
			t,
			adminNode.base,
			readerPeer.wallet.address,
			{ expectedLicenseCount: 2 }
		);
		t.ok(
			b4a.equals(decodedReaderAfterWhitelist.balance, initialReaderBalance),
			'reader balance preserved after whitelist append'
		);

		const adminNodeEntryAfter = await adminNode.base.view.get(adminNode.wallet.address);
		const decodedAdminAfter = nodeEntryUtils.decode(adminNodeEntryAfter.value);
		t.ok(decodedAdminAfter, 'admin node entry decodes after whitelist append');
		t.ok(
			b4a.equals(decodedAdminAfter.balance, adminBalanceSnapshot),
			'admin balance remains unchanged while initialization enabled'
		);

		const txEntry = await adminNode.base.view.get(whitelistTxHash);
		t.ok(txEntry, 'whitelist transaction recorded to prevent replays');

		await context.sync();

		const { decodedEntry: readerReplicaEntry } = await assertReaderWhitelisted(
			t,
			readerPeer.base,
			readerPeer.wallet.address,
			{ expectedLicenseCount: 2 }
		);
		t.ok(
			b4a.equals(readerReplicaEntry.balance, initialReaderBalance),
			'reader balance replicated with whitelist role'
		);
	});
}
