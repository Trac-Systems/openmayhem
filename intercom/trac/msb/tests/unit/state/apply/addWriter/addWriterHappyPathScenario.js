import { test } from 'brittle';
import b4a from 'b4a';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import {
	setupAddWriterScenario,
	selectWriterPeer,
	buildAddWriterPayload,
	assertAddWriterSuccessState
} from './addWriterScenarioHelpers.js';

export default function addWriterHappyPathScenario() {
	test('State.apply addWriter promotes whitelisted nodes via validator cosignature - happy path', async t => {
		const context = await setupAddWriterScenario(t);
		const validatorPeer = context.adminBootstrap;
		const writerPeer = selectWriterPeer(context);

		const adminEntryBefore = await validatorPeer.base.view.get(validatorPeer.wallet.address);
		t.ok(adminEntryBefore, 'admin entry exists before addWriter');
		const decodedAdminBefore = nodeEntryUtils.decode(adminEntryBefore.value);
		t.ok(decodedAdminBefore, 'admin entry decodes before addWriter');
		const adminBalanceBefore = b4a.from(decodedAdminBefore.balance);

		const payload = await buildAddWriterPayload(context, {
			readerPeer: writerPeer,
			validatorPeer
		});

		await validatorPeer.base.append(payload);
		await validatorPeer.base.update();
		await eventFlush();

		await assertAddWriterSuccessState(t, context, {
			readerPeer: writerPeer,
			validatorPeer,
			writerInitialBalance: context.addWriterScenario?.writerInitialBalance,
			validatorBalanceBefore: adminBalanceBefore,
			payload
		});
	});
}
