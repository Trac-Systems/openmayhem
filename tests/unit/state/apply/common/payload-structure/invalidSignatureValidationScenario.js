import b4a from 'b4a';
import { eventFlush } from '../../../../../helpers/autobaseTestHelpers.js';
import {
	safeDecodeApplyOperation,
	safeEncodeApplyOperation
} from '../../../../../../src/utils/protobuf/operationHelpers.js';
import OperationValidationScenarioBase from '../base/OperationValidationScenarioBase.js';
import { createSignature } from '../../../../../helpers/createTestSignature.js';

export const SignatureMutationStrategy = {
	ZERO_FILL: 0,
	FOREIGN_SIGNATURE: 1,
	TYPE_MISMATCH: 2,
	AMOUNT_SIGNATURE: 3
};

export default class InvalidSignatureValidationScenario extends OperationValidationScenarioBase {
	constructor({
		title,
		setupScenario,
		buildValidPayload,
		assertStateUnchanged,
		strategy = SignatureMutationStrategy.FOREIGN_SIGNATURE,
		applyInvalidPayload = defaultApplyInvalidPayload,
		expectedLogs
	}) {
		const mutatePayload = createMutationStrategy(strategy);
		super({
			title,
			setupScenario,
			buildValidPayload,
			mutatePayload,
			applyInvalidPayload,
			assertStateUnchanged,
			expectedLogs
		});
	}
}

function createMutationStrategy(strategy) {
	switch (strategy) {
		case SignatureMutationStrategy.ZERO_FILL:
			return zeroFillSignature;
		case SignatureMutationStrategy.TYPE_MISMATCH:
			return typeMismatchSignature;
		case SignatureMutationStrategy.AMOUNT_SIGNATURE:
			return mutateAmountSignature;
		case SignatureMutationStrategy.FOREIGN_SIGNATURE:
		default:
			return foreignSignature;
	}
}

async function foreignSignature(t, validPayload) {
	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'fixtures decode');

	const signatureParent = findSignatureParent(decodedPayload);
	if (!signatureParent || !signatureParent.parent?.tx) return validPayload;

	const { signature } = await createSignature(signatureParent.parent.tx);
	if (signature?.length === signatureParent.parent.is.length) {
		signatureParent.parent.is = signature;
	} else {
		signatureParent.parent.is = b4a.alloc(signatureParent.parent.is.length);
	}

	return safeEncodeApplyOperation(decodedPayload);
}

async function zeroFillSignature(t, validPayload) {
	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'fixtures decode');

	const signatureParent = findSignatureParent(decodedPayload);
	if (!signatureParent) return validPayload;

	if (!signatureParent.parent?.tx) return validPayload;
	const { signature } = await createSignature(signatureParent.parent.tx);
	if (signature?.length === signatureParent.parent.is.length) {
		signatureParent.parent.is = signature;
	} else {
		signatureParent.parent.is = b4a.alloc(signatureParent.parent.is.length);
	}
	return safeEncodeApplyOperation(decodedPayload);
}

async function typeMismatchSignature(t, validPayload) {
	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'fixtures decode');

	const signatureParent = findSignatureParent(decodedPayload);
	if (!signatureParent) return validPayload;

	if (!signatureParent.parent?.tx) return validPayload;
	const { signature } = await createSignature(signatureParent.parent.tx);
	if (signature?.length === signatureParent.parent.is.length) {
		const mutatedSignature = Buffer.from(signature);
		mutatedSignature[mutatedSignature.length - 1] ^= 0xff;
		signatureParent.parent.is = mutatedSignature;
	} else {
		signatureParent.parent.is = Buffer.from((signatureParent.parent.is.length).toString());
	}
	return safeEncodeApplyOperation(decodedPayload);
}

async function mutateAmountSignature(t, validPayload) {
	const decodedPayload = safeDecodeApplyOperation(validPayload);
	t.ok(decodedPayload, 'fixtures decode');

	const parent = findSignatureParent(decodedPayload);
	if (!parent?.parent?.am) return validPayload;

	if (!parent.parent.tx) return validPayload;
	const { signature } = await createSignature(parent.parent.tx);
	if (signature?.length !== parent.parent.is.length) return validPayload;

	const mutatedAmount = b4a.from(parent.parent.am);
	mutatedAmount[mutatedAmount.length - 1] ^= 0xff;
	parent.parent.am = mutatedAmount;
	parent.parent.is = signature;

	return safeEncodeApplyOperation(decodedPayload);
}

function findSignatureParent(payload) {
	const signatureParents = ['cao', 'bio', 'aco', 'tro', 'rao', 'bdo', 'txo'];
	for (const key of signatureParents) {
		const parent = payload[key];
		if (parent?.is) {
			return { parent, key };
		}
	}
	return null;
}

async function defaultApplyInvalidPayload(context, invalidPayload) {
	const { bootstrap } = context;
	await bootstrap.base.append(invalidPayload);
	await bootstrap.base.update();
	await eventFlush();
}
