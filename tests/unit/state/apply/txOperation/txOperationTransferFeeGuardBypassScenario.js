import { Status } from '../../../../../src/core/state/utils/transaction.js';
import { createTransferFeeGuardScenario } from './txOperationTransferFeeGuardScenarioFactory.js';

function createBypassScenario({ title, logMessage, status }) {
	return createTransferFeeGuardScenario({
		title,
		expectedLogs: [logMessage],
		applyPatch: async ({ node }) => {
			console.error(logMessage);
			return {
				skipAppend: true,
				cleanup: () => {},
				status
			};
		}
	});
}

export function txOperationTransferFeeResultNullScenario() {
	return createBypassScenario({
		title: 'State.apply txOperation rejects payloads when fee transfer result is null',
		logMessage: 'Fee transfer operation failed completely.',
		status: Status.FAILURE
	});
}

export function txOperationTransferFeeResultIgnoredScenario() {
	return createBypassScenario({
		title: 'State.apply txOperation skips payloads when fee transfer result is ignored',
		logMessage: 'Fee transfer operation skipped.',
		status: Status.IGNORE
	});
}

export function txOperationTransferFeeRequesterEntryMissingScenario() {
	return createBypassScenario({
		title: 'State.apply txOperation rejects payloads when requester fee entry is missing',
		logMessage: 'Failed to process requester fee deduction.',
		status: Status.FAILURE
	});
}

export function txOperationTransferFeeValidatorEntryMissingScenario() {
	return createBypassScenario({
		title: 'State.apply txOperation rejects payloads when validator fee entry is missing',
		logMessage: 'Failed to process validator fee reward.',
		status: Status.FAILURE
	});
}
