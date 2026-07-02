import b4a from 'b4a';
import { test } from 'brittle';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import {
	setupTransferScenario,
	buildTransferPayload,
	snapshotTransferEntries,
	assertTransferFailureState
} from './transferScenarioHelpers.js';

export default function transferInvalidIncomingDataScenario() {
	test('State.apply transfer rejects when incoming data is missing', async t => {
		const context = await setupTransferScenario(t);
		const { senderPeer, recipientPeer, validatorPeer } = context.transferScenario;

		const snapshots = await snapshotTransferEntries(context, {
			senderPeer,
			recipientPeer,
			validatorPeer
		});

		const payload = await buildTransferPayload(context);
		const decoded = safeDecodeApplyOperation(payload);
		t.ok(decoded?.address && decoded?.tro?.to, 'transfer payload decodes');

		const requesterAddressBuffer = decoded.address;
		const recipientAddressBuffer = decoded.tro.to;

		const originalEquals = b4a.equals;
		const originalConsoleError = console.error;
		const capturedLogs = [];

		console.error = (...args) => {
			capturedLogs.push(args);
			originalConsoleError(...args);
		};

		b4a.equals = (a, b) => {
			if (originalEquals(a, requesterAddressBuffer) && originalEquals(b, recipientAddressBuffer)) {
				return undefined;
			}
			return originalEquals(a, b);
		};

		try {
			await validatorPeer.base.append(payload);
			await validatorPeer.base.update();
			await eventFlush();
		} finally {
			b4a.equals = originalEquals;
			console.error = originalConsoleError;
		}

	await assertTransferFailureState(t, context, {
				payload,
				senderEntryBefore: snapshots.senderEntry,
				recipientEntryBefore: snapshots.recipientEntry
			});

		const expectedLog = capturedLogs.some(args =>
			args.some(arg => typeof arg === 'string' && arg.includes('Invalid transfer incoming data.'))
		);
		t.ok(expectedLog, 'expected apply log was emitted');
	});
}
