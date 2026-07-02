import RequesterBalanceScenarioBase from './base/requesterBalanceScenarioBase.js';

export default class RequesterBalanceFeeApplicationFailureScenario extends RequesterBalanceScenarioBase {
	constructor(options) {
		super({
			...options,
			failNextBalanceSub: true,
			expectedLogs: options?.expectedLogs ?? ['Failed to apply fee to requester balance.']
		});
	}
}
