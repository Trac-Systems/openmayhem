import { test } from 'brittle';
import b4a from 'b4a';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import setupAppendWhitelistScenario, {
	buildAppendWhitelistPayload,
	selectReaderPeer,
	assertReaderWhitelisted
} from './appendWhitelistScenarioHelpers.js';

export default function appendWhitelistHappyPathScenario() {
	test('State.apply appendWhitelist registers reader nodes and assigns licenses - happy path', async t => {
		const context = await setupAppendWhitelistScenario(t);
		const adminNode = context.adminBootstrap;
		const readerPeer = selectReaderPeer(context);

		const adminNodeEntryBefore = await adminNode.base.view.get(adminNode.wallet.address);
		t.ok(adminNodeEntryBefore, 'admin node entry exists');
		const decodedAdminBefore = nodeEntryUtils.decode(adminNodeEntryBefore.value);
		t.ok(decodedAdminBefore, 'admin node entry decodes');
		const adminBalanceSnapshot = decodedAdminBefore.balance && b4a.from(decodedAdminBefore.balance);

		const payload = await buildAppendWhitelistPayload(context, readerPeer.wallet.address);
		const decodedPayload = safeDecodeApplyOperation(payload);
		t.ok(decodedPayload, 'payload decodes');
		const whitelistTxHash = decodedPayload?.aco?.tx?.toString('hex');
		t.ok(whitelistTxHash, 'whitelist tx hash extracted');

		await adminNode.base.append(payload);
		await adminNode.base.update();
		await eventFlush();

		await assertReaderWhitelisted(t, adminNode.base, readerPeer.wallet.address, {
			expectedLicenseCount: 2
		});

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

		await assertReaderWhitelisted(t, readerPeer.base, readerPeer.wallet.address, {
			expectedLicenseCount: 2
		});
	});
}
