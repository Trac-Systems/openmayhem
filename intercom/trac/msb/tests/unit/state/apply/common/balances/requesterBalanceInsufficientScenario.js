import b4a from 'b4a';
import RequesterBalanceScenarioBase from './base/requesterBalanceScenarioBase.js';

export default class RequesterBalanceInsufficientScenario extends RequesterBalanceScenarioBase {
	constructor(options) {
		super({
			...options,
			mutateDecodedEntry: decoded => ({
				...decoded,
				balance: b4a.alloc(decoded.balance.length)
			}),
			expectedLogs: ['Insufficient requester balance.']
		});
	}
}
