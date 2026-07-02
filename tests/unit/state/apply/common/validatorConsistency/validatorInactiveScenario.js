import b4a from 'b4a';
import { WRITER_MASK } from '../../../../../../src/core/state/utils/roles.js';
import ValidatorConsistencyScenarioBase from './base/validatorConsistencyScenarioBase.js';

export default class ValidatorInactiveScenario extends ValidatorConsistencyScenarioBase {
	constructor(options) {
		super({
			...options,
			mutateEntry: entry => {
				if (!entry.value) {
					throw new Error('Validator inactive scenario requires an existing validator entry.');
				}
				const mutated = b4a.from(entry.value);
				mutated[0] &= ~WRITER_MASK;
				return { ...entry, value: mutated };
			}
		});
	}
}
