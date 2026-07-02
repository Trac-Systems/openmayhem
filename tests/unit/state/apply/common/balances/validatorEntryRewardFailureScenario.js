import ValidatorEntryValidationScenarioBase from '../validatorEntryValidation/base/validatorEntryValidationScenarioBase.js';

export default class ValidatorEntryRewardFailureScenario extends ValidatorEntryValidationScenarioBase {
	constructor(options) {
		super({
			...options,
			failNextBalanceAdd: true,
			expectedLogs: options?.expectedLogs ?? ['Failed to transfer fee to validator.']
		});
	}
}
