import RequesterBalanceScenarioBase from './base/requesterBalanceScenarioBase.js';

export default class RequesterBalanceUpdateFailureScenario extends RequesterBalanceScenarioBase {
	constructor(options) {
		super({
			...options,
			failNextBalanceUpdate: true,
			expectedLogs: options?.expectedLogs ?? ['Failed to update node balance.']
		});
	}
}
