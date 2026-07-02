import { test } from 'brittle';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import setupDisableInitializationScenario, {
	buildDisableInitializationPayload,
	assertInitializationDisabledState
} from './disableInitializationScenarioHelpers.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';

export default function disableInitializationAlreadyDisabledScenario() {
	test('State.apply disableInitialization aborts when initialization already disabled', async t => {
		const context = await setupDisableInitializationScenario(t);
		const adminNode = context.adminBootstrap;
		const readerNode = context.peers[1];

		const firstPayload = await buildDisableInitializationPayload(context);
		await adminNode.base.append(firstPayload);
		await adminNode.base.update();
		await eventFlush();
		await assertInitializationDisabledState(t, adminNode.base, firstPayload);

		const expectedLog = 'Balance initialization already disabled.';
		const capturedLogs = [];
		const originalConsoleError = console.error;
		console.error = (...args) => {
			capturedLogs.push(args);
			originalConsoleError(...args);
		};

		const secondPayload = await buildDisableInitializationPayload(context);
		try {
			await adminNode.base.append(secondPayload);
			await adminNode.base.update();
			await eventFlush();
		} finally {
			console.error = originalConsoleError;
		}

		const decodedSecondPayload = safeDecodeApplyOperation(secondPayload);
		t.ok(decodedSecondPayload?.cao?.tx, 'fixtures decode');
		const secondTxKey = decodedSecondPayload.cao.tx.toString('hex');
		const secondTxEntry = await adminNode.base.view.get(secondTxKey);
		t.is(secondTxEntry, null, 'second disable tx not recorded');

		await assertInitializationDisabledState(t, adminNode.base);
		await context.sync();
		await assertInitializationDisabledState(t, readerNode.base);

		const logFound = capturedLogs.some(args =>
			args.some(arg => typeof arg === 'string' && arg.includes(expectedLog))
		);
		t.ok(logFound, `expected apply log "${expectedLog}" was emitted`);
	});
}
