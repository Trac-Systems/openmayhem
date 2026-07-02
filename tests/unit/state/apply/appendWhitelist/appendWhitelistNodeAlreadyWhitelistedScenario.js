import { test } from 'brittle';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import setupAppendWhitelistScenario, {
	buildAppendWhitelistPayload,
	selectReaderPeer,
	assertReaderWhitelisted
} from './appendWhitelistScenarioHelpers.js';

export default function appendWhitelistNodeAlreadyWhitelistedScenario() {
	test('State.apply appendWhitelist rejects nodes that are already whitelisted', async t => {
		const context = await setupAppendWhitelistScenario(t);
		const adminNode = context.adminBootstrap;
		const readerPeer = selectReaderPeer(context);

		const firstPayload = await buildAppendWhitelistPayload(
			context,
			readerPeer.wallet.address
		);
		await adminNode.base.append(firstPayload);
		await adminNode.base.update();
		await eventFlush();

		await assertReaderWhitelisted(t, adminNode.base, readerPeer.wallet.address, {
			expectedLicenseCount: 2
		});

		const duplicatePayload = await buildAppendWhitelistPayload(
			context,
			readerPeer.wallet.address
		);

		const capturedLogs = [];
		const originalConsoleError = console.error;
		console.error = (...args) => {
			capturedLogs.push(args);
			originalConsoleError(...args);
		};

		try {
			await adminNode.base.append(duplicatePayload);
			await adminNode.base.update();
			await eventFlush();
		} finally {
			console.error = originalConsoleError;
		}

		await assertReaderWhitelisted(t, adminNode.base, readerPeer.wallet.address, {
			expectedLicenseCount: 2
		});
		await context.sync();
		await assertReaderWhitelisted(t, readerPeer.base, readerPeer.wallet.address, {
			expectedLicenseCount: 2
		});

		const foundLog = capturedLogs.some(args =>
			args.some(arg => typeof arg === 'string' && arg.includes('Node already whitelisted.'))
		);
		t.ok(foundLog, 'expected apply log "Node already whitelisted." was emitted');
	});
}
