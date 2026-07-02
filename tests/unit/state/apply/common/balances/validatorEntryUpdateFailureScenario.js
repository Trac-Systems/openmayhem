import ValidatorEntryValidationScenarioBase from '../validatorEntryValidation/base/validatorEntryValidationScenarioBase.js';

export default class ValidatorEntryUpdateFailureScenario extends ValidatorEntryValidationScenarioBase {
	constructor(options) {
		super({
			...options,
			failNextBalanceUpdate: true,
			expectedLogs: options?.expectedLogs ?? ['Failed to update validator entry.']
		});
	}
}
