import { test } from 'brittle';
import b4a from 'b4a';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { selectWriterPeer } from '../addWriter/addWriterScenarioHelpers.js';
import {
	setupBanValidatorScenario,
	buildBanValidatorPayload,
	assertBanValidatorSuccessState
} from './banValidatorScenarioHelpers.js';
import nodeEntryUtils, { ZERO_LICENSE } from '../../../../../src/core/state/utils/nodeEntry.js';
import { BALANCE_ZERO, toBalance } from '../../../../../src/core/state/utils/balance.js';

export default function banValidatorWhitelistedZeroBalanceScenario() {
	test('State.apply banValidator handles zero-balance whitelisted non-writer without unstake', async t => {
		const context = await setupBanValidatorScenario(t, { promoteToWriter: false });
		const adminPeer = context.adminBootstrap;
		const targetPeer = context.banValidatorScenario?.validatorPeer ?? selectWriterPeer(context);

		const entryBefore = await adminPeer.base.view.get(targetPeer.wallet.address);
		t.ok(entryBefore, 'whitelisted node entry exists before banValidator');
		const decodedBefore = nodeEntryUtils.decode(entryBefore?.value);
		t.ok(decodedBefore, 'whitelisted node entry decodes before banValidator');
		if (!decodedBefore) return;

		const balanceBefore = toBalance(decodedBefore.balance);
		const stakedBefore = toBalance(decodedBefore.stakedBalance);
		t.ok(balanceBefore, 'balance before banValidator decodes');
		t.ok(stakedBefore, 'staked balance before banValidator decodes');
		t.ok(b4a.equals(balanceBefore.value, BALANCE_ZERO.value), 'balance is zero before ban');
		t.ok(b4a.equals(stakedBefore.value, BALANCE_ZERO.value), 'staked balance is zero before ban');
		t.ok(!b4a.equals(decodedBefore.license, ZERO_LICENSE), 'license present before ban');

		const adminEntryBefore = await adminPeer.base.view.get(adminPeer.wallet.address);
		t.ok(adminEntryBefore, 'admin entry exists before banValidator');

		const payload = await buildBanValidatorPayload(context, { adminPeer, validatorPeer: targetPeer });

		await adminPeer.base.append(payload);
		await adminPeer.base.update();
		await eventFlush();

		await assertBanValidatorSuccessState(t, context, {
			validatorPeer: targetPeer,
			adminPeer,
			validatorEntryBefore: entryBefore,
			adminEntryBefore,
			payload,
			expectedInitialRoles: { isWhitelisted: true, isWriter: false, isIndexer: false },
			expectWriterRegistry: false
		});

		const entryAfter = await adminPeer.base.view.get(targetPeer.wallet.address);
		t.ok(entryAfter, 'node entry exists after banValidator');
		const decodedAfter = nodeEntryUtils.decode(entryAfter?.value);
		t.ok(decodedAfter, 'node entry decodes after banValidator');
		if (decodedAfter) {
			t.ok(
				b4a.equals(decodedAfter.balance, BALANCE_ZERO.value),
				'balance remains zero after banValidator'
			);
			t.ok(
				b4a.equals(decodedAfter.stakedBalance, BALANCE_ZERO.value),
				'staked balance remains zero after banValidator'
			);
			t.ok(
				b4a.equals(decodedAfter.license, decodedBefore.license),
				'license preserved after banValidator'
			);
		}

		await context.sync();
		const replicated = await targetPeer.base.view.get(targetPeer.wallet.address);
		t.ok(replicated, 'replicated entry exists after banValidator');
		const decodedReplicated = nodeEntryUtils.decode(replicated?.value);
		t.ok(decodedReplicated, 'replicated entry decodes after banValidator');
		if (decodedReplicated) {
			t.ok(
				b4a.equals(decodedReplicated.balance, BALANCE_ZERO.value),
				'replicated balance remains zero'
			);
			t.ok(
				b4a.equals(decodedReplicated.stakedBalance, BALANCE_ZERO.value),
				'replicated staked balance remains zero'
			);
			t.ok(
				b4a.equals(decodedReplicated.license, decodedBefore.license),
				'replicated license unchanged'
			);
		}
	});
}
