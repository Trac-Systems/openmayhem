import { test } from 'brittle';
import b4a from 'b4a';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { toBalance, BALANCE_FEE } from '../../../../../src/core/state/utils/balance.js';
import setupAppendWhitelistScenario, {
	buildAppendWhitelistPayload,
	selectReaderPeer,
	assertReaderWhitelisted
} from './appendWhitelistScenarioHelpers.js';
import {
	buildDisableInitializationPayload,
	assertInitializationDisabledState
} from '../disableInitialization/disableInitializationScenarioHelpers.js';

export default function appendWhitelistFeeAfterDisableScenario() {
	test('State.apply appendWhitelist charges admin fee once initialization is disabled', async t => {
		const context = await setupAppendWhitelistScenario(t);
		const adminNode = context.adminBootstrap;
		const readerPeer = selectReaderPeer(context);

		const disablePayload = await buildDisableInitializationPayload(context);
		await adminNode.base.append(disablePayload);
		await adminNode.base.update();
		await eventFlush();
		await assertInitializationDisabledState(t, adminNode.base, disablePayload);

		const adminEntryBefore = await adminNode.base.view.get(adminNode.wallet.address);
		t.ok(adminEntryBefore, 'admin node entry exists after disabling initialization');
		const decodedAdminBefore = nodeEntryUtils.decode(adminEntryBefore.value);
		t.ok(decodedAdminBefore, 'admin node entry decodes before fee is applied');
		const adminBalanceBefore = toBalance(decodedAdminBefore.balance);
		t.ok(adminBalanceBefore, 'admin balance decodes');

		const whitelistPayload = await buildAppendWhitelistPayload(
			context,
			readerPeer.wallet.address
		);
		await adminNode.base.append(whitelistPayload);
		await adminNode.base.update();
		await eventFlush();

		await assertReaderWhitelisted(t, adminNode.base, readerPeer.wallet.address, {
			expectedLicenseCount: 2
		});

		const adminEntryAfter = await adminNode.base.view.get(adminNode.wallet.address);
		const decodedAdminAfter = nodeEntryUtils.decode(adminEntryAfter.value);
		t.ok(decodedAdminAfter, 'admin node entry decodes after fee is applied');
		const adminBalanceAfter = toBalance(decodedAdminAfter.balance);
		t.ok(adminBalanceAfter, 'admin balance decodes after fee is applied');

		const expectedBalance = adminBalanceBefore.sub(BALANCE_FEE);
		t.ok(expectedBalance, 'fee subtraction succeeds');
		t.ok(
			b4a.equals(adminBalanceAfter.value, expectedBalance.value),
			'admin balance reduced by BALANCE_FEE when initialization disabled'
		);

		await context.sync();

		await assertReaderWhitelisted(t, readerPeer.base, readerPeer.wallet.address, {
			expectedLicenseCount: 2
		});
	});
}
