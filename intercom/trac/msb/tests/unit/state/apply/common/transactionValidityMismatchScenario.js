import b4a from 'b4a';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import OperationValidationScenarioBase from './base/OperationValidationScenarioBase.js';

export default class TransactionValidityMismatchScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		txValidityPath = ['cao', 'txv'],
		applyInvalidPayload = defaultApplyInvalidPayload,
		rebuildPayloadWithTxValidity,
		expectedLogs
	}) {
		if (typeof rebuildPayloadWithTxValidity !== 'function') {
			throw new Error(
				'Transaction validity mismatch scenario requires a rebuildPayloadWithTxValidity function.'
			);
		}

		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload: (t, payload, context) =>
				mutateTxValidityPayload({
					t,
					validPayload: payload,
					context,
					txValidityPath,
					rebuildPayloadWithTxValidity
				}),
			applyInvalidPayload,
			assertStateUnchanged,
			expectedLogs
		});
	}
}

async function mutateTxValidityPayload({
	t,
	validPayload,
	context,
	txValidityPath,
	rebuildPayloadWithTxValidity
}) {
	const path = Array.isArray(txValidityPath) ? txValidityPath : [txValidityPath];
	if (!path.length) {
		throw new Error('Transaction validity mutation requires a non-empty path.');
	}

	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'fixtures decode');

	const txValidityBuffer = locateTxValidityBuffer(decodedPayload, path);
	if (!txValidityBuffer) {
		return validPayload;
	}

	const mutatedValidity = flipTxValidity(txValidityBuffer);
	return rebuildPayloadWithTxValidity({
		context,
		t,
		validPayload,
		mutatedTxValidity: mutatedValidity
	});
}

function locateTxValidityBuffer(payload, path) {
	let current = payload;
	for (let i = 0; i < path.length - 1; i++) {
		if (!current) return null;
		current = current[path[i]];
	}

	const field = path[path.length - 1];
	const value = current?.[field];
	if (!b4a.isBuffer(value) || value.length === 0) {
		return null;
	}
	return value;
}

function flipTxValidity(txValidityBuffer) {
	const mutatedValidity = b4a.from(txValidityBuffer);
	const lastIndex = mutatedValidity.length - 1;
	mutatedValidity[lastIndex] ^= 0xff;
	return mutatedValidity;
}

async function defaultApplyInvalidPayload(context, invalidPayload) {
	const { bootstrap } = context;
	await bootstrap.base.append(invalidPayload);
	await bootstrap.base.update();
	await eventFlush();
}
