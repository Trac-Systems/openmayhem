import b4a from 'b4a';
import OperationValidationScenarioBase from '../../base/OperationValidationScenarioBase.js';
import { safeDecodeApplyOperation } from '../../../../../../../src/utils/protobuf/operationHelpers.js';
import addressUtils from '../../../../../../../src/core/state/utils/address.js';
import { eventFlush } from '../../../../../../helpers/autobaseTestHelpers.js';
import { config } from '../../../../../../helpers/config.js'

export const ValidatorEntryMutation = {
	DELETE: Symbol('validator-entry-delete')
};

const DEFAULT_VALIDATOR_ADDRESS_PATH = ['rao', 'va'];

export default class ValidatorConsistencyScenarioBase extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		mutateEntry,
		selectNode = defaultSelectNode,
		validatorAddressPath = DEFAULT_VALIDATOR_ADDRESS_PATH,
		expectedLogs,
		applyInvalidPayload
	}) {
		if (typeof mutateEntry !== 'function') {
			throw new Error('Validator consistency scenario requires a mutateEntry function.');
		}

		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload: passThroughPayload,
			applyInvalidPayload:
				typeof applyInvalidPayload === 'function'
					? applyInvalidPayload
					: createApplyInvalidPayload({
							selectNode,
							validatorAddressPath,
							mutateEntry
					  }),
			assertStateUnchanged,
			expectedLogs
		});
	}
}

function passThroughPayload(_t, payload) {
	return payload;
}

function defaultSelectNode(context) {
	return context.adminBootstrap ?? context.bootstrap ?? context.peers?.[0] ?? null;
}

function createApplyInvalidPayload({ selectNode, validatorAddressPath, mutateEntry }) {
	return async (context, payload, t, validPayload) => {
		const node = selectNode(context);
		if (!node?.base) {
			throw new Error('Validator consistency scenario requires a writable node.');
		}

		const validatorAddress = extractValidatorAddress(validPayload ?? payload, validatorAddressPath);
		if (!validatorAddress) {
			throw new Error('Validator address could not be derived from payload.');
		}

		const cleanup = patchValidatorEntry({
			base: node.base,
			mutateEntry,
			context,
			t,
			validatorAddressString: validatorAddress.string,
			validatorAddressBuffer: validatorAddress.buffer
		});

		try {
			await node.base.append(payload);
			await node.base.update();
			await eventFlush();
		} finally {
			await cleanup();
		}
	};
}

function patchValidatorEntry({
	base,
	mutateEntry,
	context,
	t,
	validatorAddressString,
	validatorAddressBuffer
}) {
	const originalApply = base._handlers.apply;
	let shouldInterceptNextApply = true;

	base._handlers.apply = async function patchedApply(nodes, view, baseCtx) {
		if (!shouldInterceptNextApply) {
			return originalApply.call(this, nodes, view, baseCtx);
		}

		shouldInterceptNextApply = false;
		const originalBatch = view.batch;
		view.batch = function patchedBatch(...args) {
			const batch = originalBatch.apply(this, args);
			if (!batch || typeof batch.get !== 'function') {
				return batch;
			}

			const originalGet = batch.get.bind(batch);
			let hasMutatedEntry = false;

			batch.get = async key => {
				if (
					!hasMutatedEntry &&
					isValidatorKeyMatch(key, validatorAddressString, validatorAddressBuffer)
				) {
					hasMutatedEntry = true;
					const entry = await originalGet(key);

					const mutation = await mutateEntry(entry, {
						context,
						t,
						validatorAddress: validatorAddressString
					});

					return applyEntryMutation(entry, mutation);
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

	return async () => {
		base._handlers.apply = originalApply;
	};
}

function applyEntryMutation(entry, mutation) {
	if (typeof mutation === 'undefined') {
		return entry;
	}

	if (mutation === ValidatorEntryMutation.DELETE) {
		return null;
	}

	if (entry && (b4a.isBuffer(mutation) || mutation instanceof Uint8Array)) {
		return {
			...entry,
			value: b4a.from(mutation)
		};
	}

	if (mutation && typeof mutation === 'object') {
		return mutation;
	}

	throw new Error('Invalid validator entry mutation result.');
}

function extractValidatorAddress(payloadBuffer, path) {
	const decoded = safeDecodeApplyOperation(payloadBuffer);
	if (!decoded) return null;

	const value = Array.isArray(path) && path.length > 0 ? traversePath(decoded, path) : null;
	if (!value || !b4a.isBuffer(value) || value.length === 0) {
		return null;
	}

	const addressString = addressUtils.bufferToAddress(value, config.addressPrefix);
	if (!addressString) {
		return null;
	}

	return { buffer: value, string: addressString };
}

function traversePath(payload, path) {
	return path.reduce((current, segment) => (current ? current[segment] : null), payload);
}

function isValidatorKeyMatch(key, targetString, targetBuffer) {
	if (typeof key === 'string') {
		return key === targetString;
	}
	if (b4a.isBuffer(key) && targetBuffer) {
		return b4a.equals(key, targetBuffer);
	}
	return false;
}
