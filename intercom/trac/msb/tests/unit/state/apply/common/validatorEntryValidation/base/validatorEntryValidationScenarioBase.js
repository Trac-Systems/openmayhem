import b4a from 'b4a';
import OperationValidationScenarioBase from '../../base/OperationValidationScenarioBase.js';
import nodeEntryUtils from '../../../../../../../src/core/state/utils/nodeEntry.js';
import { safeDecodeApplyOperation } from '../../../../../../../src/utils/protobuf/operationHelpers.js';
import addressUtils from '../../../../../../../src/core/state/utils/address.js';
import { eventFlush } from '../../../../../../helpers/autobaseTestHelpers.js';
import { BALANCE_ZERO } from '../../../../../../../src/core/state/utils/balance.js';
import { config } from '../../../../../../helpers/config.js'

const DEFAULT_VALIDATOR_ADDRESS_PATH = ['rao', 'va'];
const VALIDATOR_ENTRY_MARK = Symbol('validator-entry-mark');
const VALIDATOR_BALANCE_MARK = Symbol('validator-balance-mark');

export default class ValidatorEntryValidationScenarioBase extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		mutateEntry = entryPassThrough,
		mutateDecodedEntry = null,
		selectNode = defaultSelectNode,
		validatorAddressPath = DEFAULT_VALIDATOR_ADDRESS_PATH,
		expectedLogs,
		mutatePayload,
		applyInvalidPayload,
		failNextBalanceAdd = false,
		failNextBalanceUpdate = false
	}) {
		if (typeof mutateEntry !== 'function') {
			throw new Error('Validator entry validation scenario requires a mutateEntry function.');
		}

		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload: mutatePayload ?? passThroughPayload,
			applyInvalidPayload:
				typeof applyInvalidPayload === 'function'
					? applyInvalidPayload
					: createApplyInvalidPayload({
							selectNode,
							validatorAddressPath,
							mutateEntry,
							mutateDecodedEntry,
							failNextBalanceAdd,
							failNextBalanceUpdate
					  }),
			assertStateUnchanged,
			expectedLogs
		});
	}
}

function passThroughPayload(_t, payload) {
	return payload;
}

function entryPassThrough(entry) {
	return entry;
}

function defaultSelectNode(context) {
	return context.adminBootstrap ?? context.bootstrap ?? context.peers?.[0] ?? null;
}

function createApplyInvalidPayload({
	selectNode,
	validatorAddressPath,
	mutateEntry,
	mutateDecodedEntry,
	failNextBalanceAdd,
	failNextBalanceUpdate
}) {
	return async (context, payload, t, validPayload) => {
		const node = selectNode(context);
		if (!node?.base) {
			throw new Error('Validator entry validation scenario requires a writable node.');
		}

		const validatorAddress = extractValidatorAddress(validPayload ?? payload, validatorAddressPath);
		if (!validatorAddress) {
			throw new Error('Validator address could not be derived from payload.');
		}

		const cleanup = patchValidatorEntry({
			base: node.base,
			mutateEntry,
			mutateDecodedEntry,
			failNextBalanceAdd,
			failNextBalanceUpdate,
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
	mutateDecodedEntry,
	failNextBalanceAdd,
	failNextBalanceUpdate,
	context,
	t,
	validatorAddressString,
	validatorAddressBuffer
}) {
	const originalApply = base._handlers.apply;
	const originalDecode = nodeEntryUtils.decode;
	const balancePrototype = Object.getPrototypeOf(BALANCE_ZERO);
	const originalAdd = balancePrototype.add;
	const originalUpdate = balancePrototype.update;

	let shouldInterceptNextApply = true;
	let targetEntryBuffer = null;
	let shouldFailNextAdd = false;
	let shouldMarkNextBalance = false;

	const setTargetEntryBuffer = buffer => {
		if (buffer && b4a.isBuffer(buffer)) {
			targetEntryBuffer = b4a.from(buffer);
		} else {
			targetEntryBuffer = null;
		}
	};

	const isTargetEntryBuffer = buffer =>
		targetEntryBuffer &&
		b4a.isBuffer(buffer) &&
		buffer.length === targetEntryBuffer.length &&
		b4a.equals(buffer, targetEntryBuffer);

	nodeEntryUtils.decode = function patchedDecode(buffer) {
		const decoded = originalDecode(buffer);
		if (isTargetEntryBuffer(buffer)) {
			markValidatorBuffer(buffer);
			shouldFailNextAdd = failNextBalanceAdd;
			shouldMarkNextBalance = failNextBalanceUpdate;

			if (mutateDecodedEntry) {
				const mutated = mutateDecodedEntry(decoded, {
					context,
					t,
					validatorAddress: validatorAddressString
				});
				if (mutated && typeof mutated === 'object') {
					return mutated;
				}
			}
		}
		return decoded;
	};

	balancePrototype.add = function patchedAdd(balance) {
		if (shouldFailNextAdd) {
			shouldFailNextAdd = false;
			return null;
		}
		const result = originalAdd.call(this, balance);
		if (shouldMarkNextBalance && result) {
			shouldMarkNextBalance = false;
			result[VALIDATOR_BALANCE_MARK] = true;
		}
		return result;
	};

	balancePrototype.update = function patchedUpdate(nodeEntryBuffer) {
		if (this?.[VALIDATOR_BALANCE_MARK]) {
			this[VALIDATOR_BALANCE_MARK] = false;
			return null;
		}
		return originalUpdate.call(this, nodeEntryBuffer);
	};

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
					if (entry?.value && b4a.isBuffer(entry.value)) {
						setTargetEntryBuffer(entry.value);
						markValidatorBuffer(entry.value);
					} else {
						setTargetEntryBuffer(null);
					}

					const mutation = await mutateEntry(entry, {
						context,
						t,
						validatorAddress: validatorAddressString
					});

					const mutatedEntry = applyEntryMutation(entry, mutation);
					if (mutatedEntry?.value && b4a.isBuffer(mutatedEntry.value)) {
						setTargetEntryBuffer(mutatedEntry.value);
						markValidatorBuffer(mutatedEntry.value);
					} else if (mutatedEntry === null) {
						setTargetEntryBuffer(null);
					}
					return mutatedEntry;
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
		nodeEntryUtils.decode = originalDecode;
		const prototype = Object.getPrototypeOf(BALANCE_ZERO);
		prototype.add = originalAdd;
		prototype.update = originalUpdate;
	};
}

function applyEntryMutation(entry, mutation) {
	if (typeof mutation === 'undefined') {
		return entry;
	}

	if (mutation === null) {
		return null;
	}

	if (entry && (b4a.isBuffer(mutation) || mutation instanceof Uint8Array)) {
		const wrapped = { ...entry, value: b4a.from(mutation) };
		markValidatorBuffer(wrapped.value);
		return wrapped;
	}

	if (mutation && typeof mutation === 'object') {
		if (mutation.value && b4a.isBuffer(mutation.value)) {
			markValidatorBuffer(mutation.value);
		}
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

function markValidatorBuffer(buffer) {
	if (buffer && typeof buffer === 'object') {
		buffer[VALIDATOR_ENTRY_MARK] = true;
	}
}
