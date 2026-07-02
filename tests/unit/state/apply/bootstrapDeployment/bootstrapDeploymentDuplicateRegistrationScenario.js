import b4a from 'b4a';
import { test } from 'brittle';
import {
	setupBootstrapDeploymentScenario,
	buildBootstrapDeploymentPayload,
	assertBootstrapDeploymentSuccessState
} from './bootstrapDeploymentScenarioHelpers.js';
import { initializeBalances, whitelistAddress } from '../common/commonScenarioHelper.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import deploymentEntryUtils from '../../../../../src/core/state/utils/deploymentEntry.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import { config } from '../../../../helpers/config.js';

async function setupDuplicateBootstrapScenario(t) {
	const context = await setupBootstrapDeploymentScenario(t, { nodes: 4 });
	const validatorPeer = context.bootstrapDeployment?.validatorPeer ?? context.peers?.[1];
	const primaryDeployer = context.bootstrapDeployment?.deployerPeer ?? context.peers?.[2];
	const secondaryDeployer = context.peers?.[3];

	if (!validatorPeer || !primaryDeployer || !secondaryDeployer) {
		throw new Error('Duplicate bootstrap scenario requires a validator and two deployers.');
	}

	const primaryEntry = await validatorPeer.base.view.get(primaryDeployer.wallet.address);
	const primaryDecoded = primaryEntry ? nodeEntryUtils.decode(primaryEntry.value) : null;
	const initialBalance = primaryDecoded?.balance ?? null;

	if (initialBalance) {
		await initializeBalances(context, [[secondaryDeployer.wallet.address, initialBalance]]);
	}
	await whitelistAddress(context, secondaryDeployer.wallet.address);
	await context.sync();

	const secondaryEntryBefore = await validatorPeer.base.view.get(secondaryDeployer.wallet.address);

	context.bootstrapDeployment.secondaryDeployer = secondaryDeployer;
	context.bootstrapDeployment.secondaryDeployerEntryBefore = secondaryEntryBefore
		? { value: b4a.from(secondaryEntryBefore.value) }
		: null;

	return context;
}

function buildDuplicatePayload(validPayload, context) {
	const decoded = safeDecodeApplyOperation(validPayload);
	if (!decoded?.bdo) return validPayload;

	const secondaryDeployer = context.bootstrapDeployment?.secondaryDeployer ?? context.peers?.[3];
	if (!secondaryDeployer) return validPayload;

	return buildBootstrapDeploymentPayload(context, {
		deployerPeer: secondaryDeployer,
		externalBootstrap: decoded.bdo.bs,
		channel: decoded.bdo.ic,
		txValidity: decoded.bdo.txv
	});
}

async function assertDuplicateBootstrapState(t, context, validPayload, invalidPayload) {
	const validatorPeer = context.bootstrapDeployment?.validatorPeer ?? context.peers?.[1];
	const secondaryDeployer = context.bootstrapDeployment?.secondaryDeployer ?? context.peers?.[3];
	const secondaryBefore = context.bootstrapDeployment?.secondaryDeployerEntryBefore?.value ?? null;

	const validDecoded = safeDecodeApplyOperation(validPayload);
	const invalidDecoded = safeDecodeApplyOperation(invalidPayload);
	t.ok(validDecoded, 'valid payload decodes for assertions');
	t.ok(invalidDecoded, 'invalid payload decodes for assertions');
	if (!validDecoded?.bdo || !invalidDecoded?.bdo) return;

	const bootstrapHex = validDecoded.bdo.bs.toString('hex');
	const firstTxHex = validDecoded.bdo.tx.toString('hex');
	const secondTxHex = invalidDecoded.bdo.tx.toString('hex');

	const deploymentKey = `deployment/${bootstrapHex}`;
	const deploymentEntry = await validatorPeer.base.view.get(deploymentKey);
	t.ok(deploymentEntry, 'deployment entry still stored');
	const decodedDeployment = deploymentEntry ? deploymentEntryUtils.decode(deploymentEntry.value, config.addressLength) : null;
	t.ok(decodedDeployment, 'deployment entry decodes');
	if (decodedDeployment?.txHash) {
		t.is(decodedDeployment.txHash.toString('hex'), firstTxHex, 'deployment entry keeps original tx hash');
	} else if (decodedDeployment) {
		t.fail('deployment entry missing tx hash');
	}

	const firstTxEntry = await validatorPeer.base.view.get(firstTxHex);
	t.ok(firstTxEntry, 'original tx hash remains recorded');

	const duplicateTxEntry = await validatorPeer.base.view.get(secondTxHex);
	t.is(duplicateTxEntry, null, 'duplicate tx hash not recorded');

	if (secondaryDeployer) {
		const after = await validatorPeer.base.view.get(secondaryDeployer.wallet.address);
		t.ok(after, 'second deployer entry exists after rejection');
		if (secondaryBefore && after?.value) {
			const beforeDecoded = nodeEntryUtils.decode(secondaryBefore);
			const afterDecoded = nodeEntryUtils.decode(after.value);
			t.ok(beforeDecoded, 'second deployer entry before decodes');
			t.ok(afterDecoded, 'second deployer entry after decodes');
			if (beforeDecoded && afterDecoded) {
				t.ok(b4a.equals(afterDecoded.balance, beforeDecoded.balance), 'second deployer balance unchanged');
				t.ok(
					b4a.equals(afterDecoded.stakedBalance, beforeDecoded.stakedBalance),
					'second deployer stake unchanged'
				);
			}
		}
	}
}

export default function bootstrapDeploymentDuplicateRegistrationScenario() {
	test('State.apply bootstrapDeployment ignores duplicate bootstrap registrations', async t => {
		const context = await setupDuplicateBootstrapScenario(t);
		const validatorPeer = context.bootstrapDeployment?.validatorPeer ?? context.peers?.[1];

		const validPayload = await buildBootstrapDeploymentPayload(context);
		const invalidPayload = await buildDuplicatePayload(validPayload, context);

		const capturedLogs = [];
		const originalConsoleError = console.error;
		console.error = (...args) => {
			capturedLogs.push(args);
			originalConsoleError(...args);
		};

		try {
			await validatorPeer.base.append(validPayload);
			await validatorPeer.base.update();
			await assertBootstrapDeploymentSuccessState(t, context, { payload: validPayload, skipSync: true });

			await validatorPeer.base.append(invalidPayload);
			await validatorPeer.base.update();

			await assertDuplicateBootstrapState(t, context, validPayload, invalidPayload);

			const foundLog = capturedLogs.some(args =>
				args.some(arg => String(arg).includes('Bootstrap already registered.'))
			);
			t.ok(foundLog, 'expected apply log "Bootstrap already registered." was emitted');
		} finally {
			console.error = originalConsoleError;
		}
	});
}
