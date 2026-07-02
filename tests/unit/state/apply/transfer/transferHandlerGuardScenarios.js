import { createTransferHandlerGuardBypassScenario } from './transferScenarioHelpers.js';

/**
 * Covers handler-level guards that react to null entries returned from #transfer.
 * These are purely handler guards, not transfer arithmetic failures.
 */
export default function transferHandlerGuardScenarios() {
	createTransferHandlerGuardBypassScenario({
		title: 'State.apply transfer rejects payloads when sender entry from transfer result is null',
		logMessage: 'Invalid sender entry.'
	}).performScenario();

	createTransferHandlerGuardBypassScenario({
		title: 'State.apply transfer rejects payloads when validator entry from transfer result is null',
		logMessage: 'Invalid validator entry.'
	}).performScenario();

	createTransferHandlerGuardBypassScenario({
		title: 'State.apply transfer rejects payloads when recipient entry from transfer result is null',
		logMessage: 'Invalid recipient entry.'
	}).performScenario();
}
