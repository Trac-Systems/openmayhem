import b4a from 'b4a';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupTxOperationScenario,
	buildTxOperationPayload,
	assertTxOperationFailureState
} from './txOperationScenarioHelpers.js';

export default function txOperationBootstrapNotRegisteredScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply txOperation rejects payloads when bootstrap is not registered',
		setupScenario: setupTxOperationScenario,
		buildValidPayload: buildTxOperationPayload,
		mutatePayload: async (t, _validPayload, context) => {
			const unregisteredBootstrap = b4a.alloc(32, 0x24);
			t.ok(context.txOperation?.msbBootstrap, 'msb bootstrap available');
			t.ok(!b4a.equals(unregisteredBootstrap, context.txOperation?.msbBootstrap), 'unregistered bootstrap differs from MSB');
			return buildTxOperationPayload(context, { externalBootstrap: unregisteredBootstrap });
		},
		applyInvalidPayload: async (context, invalidPayload, t, validPayload) => {
			const node = context.txOperation?.validatorPeer ?? context.peers?.[1];
			const payload = validPayload ?? invalidPayload;
			const decoded = safeDecodeApplyOperation(payload);
			t.ok(decoded?.txo?.bs, 'payload exposes bootstrap key');
			const bootstrapKey = decoded?.txo?.bs?.toString('hex');
			if (!bootstrapKey) return;

			const deploymentKey = `${EntryType.DEPLOYMENT}${bootstrapKey}`;
			const originalApply = node.base._handlers.apply;

			node.base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
				const originalBatch = view.batch;
				view.batch = function patchedBatch(...args) {
					const batch = originalBatch.apply(this, args);
					if (!batch || typeof batch.get !== 'function') return batch;
					const originalGet = batch.get.bind(batch);
					batch.get = async key => {
						if (key === deploymentKey) return null;
						if (b4a.isBuffer(key) && b4a.equals(key, decoded.txo.bs)) return null;
						return originalGet(key);
					};
					return batch;
				};

				try {
					return await originalApply.call(this, nodes, view, baseCtx);
				} finally {
					view.batch = originalBatch;
				}
			};

			try {
				await node.base.append(payload);
				await node.base.update();
				await eventFlush();
			} finally {
				node.base._handlers.apply = originalApply;
			}
		},
		assertStateUnchanged: (t, context, _valid, invalidPayload) =>
			assertTxOperationFailureState(t, context, {
				payload: invalidPayload,
				validatorEntryBefore: null
			}),
		expectedLogs: ['Bootstrap has not been registered.']
	}).performScenario();
}
