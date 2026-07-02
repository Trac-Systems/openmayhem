import b4a from 'b4a';
import ValidatorEntryValidationScenarioBase from '../validatorEntryValidation/base/validatorEntryValidationScenarioBase.js';

export default class ValidatorEntryDecodeFailureScenario extends ValidatorEntryValidationScenarioBase {
	constructor(options) {
		super({
			...options,
			mutateEntry: entry => {
				if (!entry?.value) {
					throw new Error('Validator entry decode scenario requires an existing entry.');
				}
				return { ...entry, value: b4a.alloc(1) };
			},
			expectedLogs: options?.expectedLogs ?? ['Failed to decode validator entry.']
		});
	}
}
