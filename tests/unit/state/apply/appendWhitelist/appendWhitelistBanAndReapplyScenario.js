import { test } from 'brittle';
import b4a from 'b4a';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import setupAppendWhitelistScenario, {
	buildAppendWhitelistPayload,
	buildBanWriterPayload,
	selectReaderPeer,
	assertReaderWhitelisted
} from './appendWhitelistScenarioHelpers.js';

export default function appendWhitelistBanAndReapplyScenario() {
	test('State.apply appendWhitelist re-applies whitelisted role after ban without reallocating license', async t => {
		const context = await setupAppendWhitelistScenario(t);
		const adminNode = context.adminBootstrap;
		const readerPeer = selectReaderPeer(context);

		const initialWhitelistPayload = await buildAppendWhitelistPayload(
			context,
			readerPeer.wallet.address
		);
		await adminNode.base.append(initialWhitelistPayload);
		await adminNode.base.update();
		await eventFlush();

		const { decodedEntry: initialWhitelistedEntry } = await assertReaderWhitelisted(
			t,
			adminNode.base,
			readerPeer.wallet.address,
			{ expectedLicenseCount: 2 }
		);
		const initialLicenseBuffer = b4a.from(initialWhitelistedEntry.license);

		const banPayload = await buildBanWriterPayload(context, readerPeer.wallet.address);
		await adminNode.base.append(banPayload);
		await adminNode.base.update();
		await eventFlush();

		const readerEntryAfterBan = await adminNode.base.view.get(readerPeer.wallet.address);
		t.ok(readerEntryAfterBan, 'reader entry persists after ban');
		const decodedAfterBan = nodeEntryUtils.decode(readerEntryAfterBan.value);
		t.ok(decodedAfterBan, 'reader entry decodes after ban');
		t.is(decodedAfterBan.isWhitelisted, false, 'reader un-whitelisted after ban');
		t.is(decodedAfterBan.isWriter, false, 'reader writer flag cleared after ban');
		t.ok(
			b4a.equals(decodedAfterBan.license, initialLicenseBuffer),
			'license retained after ban'
		);

		const reapplyWhitelistPayload = await buildAppendWhitelistPayload(
			context,
			readerPeer.wallet.address
		);
		await adminNode.base.append(reapplyWhitelistPayload);
		await adminNode.base.update();
		await eventFlush();

		const { decodedEntry: rewhitelistedEntry } = await assertReaderWhitelisted(
			t,
			adminNode.base,
			readerPeer.wallet.address,
			{ expectedLicenseCount: 2 }
		);
		t.ok(
			b4a.equals(rewhitelistedEntry.license, initialLicenseBuffer),
			'license reused when reader re-whitelisted'
		);

		await context.sync();

		const syncedEntry = await readerPeer.base.view.get(readerPeer.wallet.address);
		t.ok(syncedEntry, 'reader entry replicated on peer');
		const decodedSyncedEntry = nodeEntryUtils.decode(syncedEntry.value);
		t.ok(decodedSyncedEntry, 'replicated entry decodes');
		t.is(decodedSyncedEntry.isWhitelisted, true, 'reader flagged as whitelisted after reapply');
		t.ok(b4a.equals(decodedSyncedEntry.license, initialLicenseBuffer),'license stable across ban/reapply cycle');
	});
}
