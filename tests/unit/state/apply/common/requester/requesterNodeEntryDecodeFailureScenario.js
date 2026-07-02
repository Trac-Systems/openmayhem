import b4a from 'b4a';
import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';
import addressUtils from '../../../../../../src/core/state/utils/address.js';
import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';
import { config } from '../../../../../helpers/config.js'

export default class RequesterNodeEntryDecodeFailureScenario extends OperationValidationScenarioBase {
	constructor({
		title = 'State.apply operation rejects payloads when requester node entry cannot be decoded',
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		expectedLogs = ['Invalid requester node entry.'],
		selectNode = context => context.bootstrap ?? context.adminBootstrap ?? context.peers?.[0],
		selectPeer = context => context.bootstrapDeployment?.deployerPeer ?? context.peers?.[1]
	}) {
		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload: (_t, payload) => payload,
			applyInvalidPayload: async (context, payload) => {
				const node = selectNode(context);
				const peer = selectPeer(context);
				if (!node?.base || !peer?.wallet?.address) {
					throw new Error('Requester node entry decode scenario requires node and peer.');
				}

				const targetAddressString = peer.wallet.address;
				const targetAddressBuffer = addressUtils.addressToBuffer(targetAddressString, config.addressPrefix);

				const originalApply = node.base._handlers.apply;
				node.base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
					const originalBatch = view.batch;
					view.batch = function patchedBatch(...args) {
						const batch = originalBatch.apply(this, args);
						if (!batch?.get) return batch;
						const originalGet = batch.get.bind(batch);
						batch.get = async key => {
							if (isTargetKey(key, targetAddressString, targetAddressBuffer)) {
								return { key, value: b4a.alloc(0) };
							}
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
			assertStateUnchanged,
			expectedLogs
		});
	}
}

function isTargetKey(key, targetAddressString, targetAddressBuffer) {
	if (typeof key === 'string') return key === targetAddressString;
	if (b4a.isBuffer(key) && targetAddressBuffer) return b4a.equals(key, targetAddressBuffer);
	return false;
}
