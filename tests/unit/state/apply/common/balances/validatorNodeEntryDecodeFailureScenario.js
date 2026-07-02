import nodeEntryUtils from '../../../../../../src/core/state/utils/nodeEntry.js';
import RequesterBalanceScenarioBase from './base/requesterBalanceScenarioBase.js';

export default class ValidatorNodeEntryDecodeFailureScenario extends RequesterBalanceScenarioBase {
	constructor({
		title = 'State.apply operation rejects payloads when validator node entry cannot be decoded',
		expectedLogs = ['Failed to decode validator entry.'],
		...options
	}) {
		super({
			...options,
			title,
			expectedLogs,
			selectPeer: options.selectPeer ?? (() => options.validatorPeer ?? null),
			mutateDecodedEntry: decoded => decoded,
			applyInvalidPayload: createApplyInvalidPayload(options)
		});
	}
}

function createApplyInvalidPayload({ selectNode, selectPeer }) {
	return async (context, payload) => {
		const node = selectNode?.(context) ?? context.bootstrap ?? context.adminBootstrap ?? context.peers?.[0];
		const peer = selectPeer?.(context) ?? context.bootstrapDeployment?.validatorPeer ?? context.peers?.[1];
		if (!node?.base || !peer?.wallet?.address) {
			throw new Error('Validator node entry decode failure scenario requires node and validator peer.');
		}

		const originalDecode = nodeEntryUtils.decode;

		nodeEntryUtils.decode = () => null;

		try {
			await node.base.append(payload);
			await node.base.update();
		} finally {
			nodeEntryUtils.decode = originalDecode;
		}
	};
}
