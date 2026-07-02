import b4a from 'b4a';
import ValidatorConsistencyScenarioBase from './base/validatorConsistencyScenarioBase.js';

export default class ValidatorWriterKeyMismatchScenario extends ValidatorConsistencyScenarioBase {
	constructor(options) {
		super({
			...options,
			mutateEntry: entry => {
				if (!entry.value) {
					throw new Error('Validator writer key mismatch scenario requires an existing validator entry.');
				}
				const mutated = b4a.from(entry.value);
				if (mutated.length > 33) mutated[1] ^= 0xff;
				return { ...entry, value: mutated };
			}
		});
	}
}
