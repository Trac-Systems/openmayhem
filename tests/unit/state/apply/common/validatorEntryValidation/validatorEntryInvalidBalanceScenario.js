import b4a from 'b4a';
import ValidatorEntryValidationScenarioBase from './base/validatorEntryValidationScenarioBase.js';

export default class ValidatorEntryInvalidBalanceScenario extends ValidatorEntryValidationScenarioBase {
	constructor(options) {
		super({
			...options,
			mutateDecodedEntry: decoded => {
				if (!decoded) return decoded;
				return {
					...decoded,
					balance: b4a.alloc(1)
				};
			},
			expectedLogs: options?.expectedLogs ?? ['Invalid validator balance.']
		});
	}
}
