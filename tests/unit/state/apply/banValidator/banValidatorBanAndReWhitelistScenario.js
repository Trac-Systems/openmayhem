import { test } from 'brittle';
import b4a from 'b4a';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	selectWriterPeer,
	buildAddWriterPayload,
	assertAddWriterSuccessState
} from '../addWriter/addWriterScenarioHelpers.js';
import {
	setupBanValidatorScenario,
	buildBanValidatorPayload,
	assertBanValidatorSuccessState
} from './banValidatorScenarioHelpers.js';
import {
	buildAppendWhitelistPayload
} from '../appendWhitelist/appendWhitelistScenarioHelpers.js';
import nodeEntryUtils, { ZERO_LICENSE } from '../../../../../src/core/state/utils/nodeEntry.js';
import addressUtils from '../../../../../src/core/state/utils/address.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import { BALANCE_ZERO, toBalance } from '../../../../../src/core/state/utils/balance.js';
import lengthEntryUtils from '../../../../../src/core/state/utils/lengthEntry.js';
import { config } from '../../../../helpers/config.js';

export default function banValidatorBanAndReWhitelistScenario() {
	test('State.apply banValidator allows re-whitelisting without changing license', async t => {
		const context = await setupBanValidatorScenario(t);
		const adminPeer = context.adminBootstrap;
		const validatorPeer = context.banValidatorScenario?.validatorPeer ?? selectWriterPeer(context);

		const validatorEntryBefore = await adminPeer.base.view.get(validatorPeer.wallet.address);
		t.ok(validatorEntryBefore, 'validator entry exists before banValidator');
		const decodedBefore = nodeEntryUtils.decode(validatorEntryBefore?.value);
		t.ok(decodedBefore, 'validator entry decodes before banValidator');
		if (!decodedBefore) return;

		const adminEntryBefore = await adminPeer.base.view.get(adminPeer.wallet.address);
		t.ok(adminEntryBefore, 'admin entry exists before banValidator');

		const banPayload = await buildBanValidatorPayload(context, { adminPeer, validatorPeer });

		await adminPeer.base.append(banPayload);
		await adminPeer.base.update();
		await eventFlush();

		await assertBanValidatorSuccessState(t, context, {
			validatorPeer,
			adminPeer,
			validatorEntryBefore,
			adminEntryBefore,
			payload: banPayload
		});

		const bannedEntry = await adminPeer.base.view.get(validatorPeer.wallet.address);
		t.ok(bannedEntry, 'validator entry exists after ban');
		const decodedBanned = nodeEntryUtils.decode(bannedEntry?.value);
		t.ok(decodedBanned, 'validator entry decodes after ban');
		if (!decodedBanned) return;
		const bannedBalance = toBalance(decodedBanned.balance);
		t.ok(bannedBalance, 'balance decodes after ban');

		const whitelistPayload = await buildAppendWhitelistPayload(
			context,
			validatorPeer.wallet.address
		);

		await adminPeer.base.append(whitelistPayload);
		await adminPeer.base.update();
		await eventFlush();

	const rewhitelistedEntry = await adminPeer.base.view.get(validatorPeer.wallet.address);
	t.ok(rewhitelistedEntry, 'validator entry exists after re-whitelist');
	const decodedRewhitelisted = nodeEntryUtils.decode(rewhitelistedEntry?.value);
	t.ok(decodedRewhitelisted, 'validator entry decodes after re-whitelist');
	if (!decodedRewhitelisted || !bannedBalance) return;

		t.is(decodedRewhitelisted.isWhitelisted, true, 'node marked whitelisted after reapply');
		t.is(decodedRewhitelisted.isWriter, false, 'writer flag remains cleared after reapply');
		t.is(decodedRewhitelisted.isIndexer, false, 'indexer flag remains cleared after reapply');
		t.ok(b4a.equals(decodedRewhitelisted.wk, decodedBefore.wk), 'writing key preserved');
		t.ok(
			b4a.equals(decodedRewhitelisted.balance, bannedBalance.value),
			'balance unchanged by re-whitelist'
		);
		t.ok(
			b4a.equals(decodedRewhitelisted.stakedBalance, BALANCE_ZERO.value),
			'staked balance remains zero after re-whitelist'
		);
		t.ok(!b4a.equals(decodedRewhitelisted.license, ZERO_LICENSE), 'license retained after reapply');
		t.ok(
			b4a.equals(decodedRewhitelisted.license, decodedBefore.license),
			'license unchanged after ban and re-whitelist'
		);

		const licenseId = decodedBefore.license.readUInt32BE();
		const licenseIndexEntry = await adminPeer.base.view.get(`${EntryType.LICENSE_INDEX}${licenseId}`);
		t.ok(licenseIndexEntry, 'license index entry persists after re-whitelist');
		const addressBuffer = addressUtils.addressToBuffer(validatorPeer.wallet.address, config.addressPrefix);
	if (licenseIndexEntry?.value && addressBuffer) {
		t.ok(
			b4a.equals(licenseIndexEntry.value, addressBuffer),
			'license index still maps to validator address'
		);
	}

	const writersLengthEntry = await adminPeer.base.view.get(EntryType.WRITERS_LENGTH);
	const writersLengthBefore = writersLengthEntry
		? lengthEntryUtils.decodeBE(writersLengthEntry.value)
		: 0;

	const addWriterPayload = await buildAddWriterPayload(context, {
		readerPeer: validatorPeer,
		validatorPeer: adminPeer
	});

	await adminPeer.base.append(addWriterPayload);
	await adminPeer.base.update();
	await eventFlush();

	await assertAddWriterSuccessState(t, context, {
		readerPeer: validatorPeer,
		validatorPeer: adminPeer,
		writerInitialBalance: decodedRewhitelisted.balance,
		expectedWriterIndex: writersLengthBefore,
		payload: addWriterPayload,
		skipSync: true
	});

	const repromotedEntry = await adminPeer.base.view.get(validatorPeer.wallet.address);
	t.ok(repromotedEntry, 'validator entry exists after re-promotion to writer');
	const decodedRepromoted = nodeEntryUtils.decode(repromotedEntry?.value);
	t.ok(decodedRepromoted, 'validator entry decodes after re-promotion to writer');
	if (decodedRepromoted) {
		t.is(decodedRepromoted.isWhitelisted, true, 'node remains whitelisted after re-promotion');
		t.is(decodedRepromoted.isWriter, true, 'writer role reassigned after re-promotion');
		t.is(decodedRepromoted.isIndexer, false, 'indexer flag stays cleared after re-promotion');
		t.ok(
			b4a.equals(decodedRepromoted.license, decodedBefore.license),
			'license unchanged after re-promotion'
		);
	}

	await context.sync();
	const replicatedEntry = await validatorPeer.base.view.get(validatorPeer.wallet.address);
	t.ok(replicatedEntry, 'replicated entry exists after re-whitelist and re-promotion');
	const decodedReplicated = nodeEntryUtils.decode(replicatedEntry?.value);
	t.ok(decodedReplicated, 'replicated entry decodes after re-whitelist and re-promotion');
	if (decodedReplicated) {
		t.is(decodedReplicated.isWhitelisted, true, 'replicated entry whitelisted');
		t.is(decodedReplicated.isWriter, true, 'replicated entry writer flag set');
		t.ok(
			b4a.equals(decodedReplicated.license, decodedBefore.license),
			'replicated entry license unchanged'
		);
	}
	});
}
