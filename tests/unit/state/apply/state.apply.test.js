import { default as test } from 'brittle';
/**
 * Note: This is not possible to cover tests with when indexer Sequence returns null. Hyperbee is shoting with errors,
 * if someone will try to do it then, this person will hack itself.
 */
async function runStateTests() {
    test.pause();
    await import('./addAdmin/state.apply.addAdmin.test.js');
	await import('./balanceInitialization/state.apply.balanceInitialization.test.js');
	await import('./disableInitialization/state.apply.disableInitialization.test.js');
	await import('./appendWhitelist/state.apply.appendWhitelist.test.js');
	await import('./addWriter/state.apply.addWriter.test.js');
	await import('./removeWriter/state.apply.removeWriter.test.js');
	await import('./addIndexer/state.apply.addIndexer.test.js');
	await import('./removeIndexer/state.apply.removeIndexer.test.js');
	await import('./banValidator/state.apply.banValidator.test.js');
	await import('./bootstrapDeployment/state.apply.bootstrapDeployment.test.js')
	await import('./txOperation/state.apply.txOperation.test.js');
	await import('./transfer/state.apply.transfer.test.js');
	await import('./adminRecovery/state.apply.adminRecovery.test.js');
	test.resume();
}

runStateTests();
