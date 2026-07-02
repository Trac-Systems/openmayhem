import b4a from 'b4a';
import RequesterBalanceScenarioBase from './base/requesterBalanceScenarioBase.js';

export default class RequesterBalanceDecodeFailureScenario extends RequesterBalanceScenarioBase {
	constructor(options) {
		super({
			...options,
			mutateDecodedEntry: decoded => ({
				...decoded,
				balance: b4a.alloc(1)
			}),
			expectedLogs: ['Invalid requester balance.']
		});
	}
}
