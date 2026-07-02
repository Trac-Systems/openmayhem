import { test } from 'brittle';
import {
	setupAdminRecoveryScenario,
	buildAdminRecoveryPayload,
	applyAdminRecovery,
	applyTransferSeries,
	assertAdminRecoverySuccessState
} from './adminRecoveryScenarioHelpers.js';
import { replicateAndSync } from '../../../../helpers/autobaseTestHelpers.js';

/**
 * Admin recovery flow with 5 nodes (1 admin, 2 indexers, 2 validators):
 * - Admin is recovered via a validator, swapping to a new writer key.
 * - A second validator processes a burst of transfers to advance indexer state.
 * - The admin entry and system indexer list reflect the new writer key (bootstrap key removed).
 */
export default function adminRecoveryHappyPathScenario() {
	test('State.apply adminRecovery updates admin writer key and indexer list', async t => {
		const context = await setupAdminRecoveryScenario(t);
		const payload = await buildAdminRecoveryPayload(context);
		await applyAdminRecovery(context, payload);

		await replicateAndSync(context.peers.map(peer => peer.base), { checkHash: false });

		// Additional traffic to advance indexer lengths/state.
		await applyTransferSeries(context);

		await assertAdminRecoverySuccessState(t, context);
	});
}
