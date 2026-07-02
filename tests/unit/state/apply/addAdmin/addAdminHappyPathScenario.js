import { test } from 'brittle';
import {
	eventFlush,
	deriveIndexerSequenceState
} from '../../../../helpers/autobaseTestHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { toTerm } from '../../../../../src/core/state/utils/balance.js';
import { applyStateMessageFactory } from '../../../../../src/messages/state/applyStateMessageFactory.js';
import { safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { setupAddAdminScenario, assertAdminState } from './addAdminScenarioHelpers.js';
import { config } from '../../../../helpers/config.js';

export default function addAdminHappyPathScenario() {
	test('State.apply addAdmin bootstraps admin node - happy path', async t => {
		const networkContext = await setupAddAdminScenario(t);
		const adminNode = networkContext.adminBootstrap;
		const readerNodes = networkContext.peers.slice(1);
		const reader = readerNodes[0];

		const txValidity = await deriveIndexerSequenceState(adminNode.base);
		const addAdminPayload = safeEncodeApplyOperation(
			await applyStateMessageFactory(adminNode.wallet, config)
				.buildCompleteAddAdminMessage(
					adminNode.wallet.address,
					adminNode.base.local.key,
					txValidity
				)
		);

		await adminNode.base.append(addAdminPayload);
		await adminNode.base.update();
		await eventFlush();

		await assertAdminState(t, adminNode.base, adminNode.wallet, adminNode.base.local.key, addAdminPayload);

		await networkContext.sync();
		await assertAdminState(t, reader.base, adminNode.wallet, adminNode.base.local.key, addAdminPayload);

		const readerNodeEntry = await reader.base.view.get(reader.wallet.address);
		t.is(readerNodeEntry, null, 'reader node remains without any role');
	});
}
