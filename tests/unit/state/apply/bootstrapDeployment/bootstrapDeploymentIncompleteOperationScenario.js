import b4a from 'b4a';
import {
	setupBootstrapDeploymentScenario,
	buildBootstrapDeploymentPayload,
	assertBootstrapDeploymentFailureState
} from './bootstrapDeploymentScenarioHelpers.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';

function removeValidatorFields(t, validPayload) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'valid payload decodes before mutation');
	if (!decoded?.bdo) return validPayload;
	const mutated = safeEncodeApplyOperation({
		type: decoded.type,
		address: decoded.address,
		bdo: {
			tx: decoded.bdo.tx,
			txv: decoded.bdo.txv,
			bs: decoded.bdo.bs,
			ic: decoded.bdo.ic,
			in: decoded.bdo.in,
			is: decoded.bdo.is
		}
	});
	const roundtrip = safeDecodeApplyOperation(mutated);
	t.ok(roundtrip, 'mutated payload decodes');
	return mutated;
}

async function assertStateUnchanged(t, context, validPayload) {
	const decoded = safeDecodeApplyOperation(validPayload);
	t.ok(decoded, 'valid payload decodes for assertions');
	if (!decoded) return;

	const { validatorEntryBefore, deployerEntryBefore, externalBootstrap } = context.bootstrapDeployment ?? {};
	const validatorPeer = context.bootstrapDeployment?.validatorPeer ?? context.peers?.[1];
	const deployerPeer = context.bootstrapDeployment?.deployerPeer ?? context.peers?.[2];

	const validatorEntryAfter = await validatorPeer.base.view.get(validatorPeer.wallet.address);
	t.ok(validatorEntryAfter, 'validator entry still exists after rejected incomplete op');
	if (validatorEntryBefore?.value && validatorEntryAfter?.value) {
		t.ok(
			b4a.equals(validatorEntryAfter.value, validatorEntryBefore.value),
			'validator entry remains unchanged after incomplete op'
		);
	}

	const deployerEntryAfter = await validatorPeer.base.view.get(deployerPeer.wallet.address);
	t.ok(deployerEntryAfter, 'deployer entry still exists after rejected incomplete op');
	if (deployerEntryBefore?.value && deployerEntryAfter?.value) {
		const before = nodeEntryUtils.decode(deployerEntryBefore.value);
		const after = nodeEntryUtils.decode(deployerEntryAfter.value);
		t.ok(before, 'deployer entry before decodes');
		t.ok(after, 'deployer entry after decodes');
		if (before && after) {
			t.ok(b4a.equals(after.balance, before.balance), 'deployer balance unchanged after incomplete op');
			t.ok(
				b4a.equals(after.stakedBalance, before.stakedBalance),
				'deployer stake unchanged after incomplete op'
			);
		}
	}

	const bootstrapBuffer = externalBootstrap ?? decoded?.bdo?.bs;
	if (bootstrapBuffer) {
		const deploymentKey = `${EntryType.DEPLOYMENT}${bootstrapBuffer.toString('hex')}`;
		const deploymentEntry = await validatorPeer.base.view.get(deploymentKey);
		t.is(deploymentEntry, null, 'deployment entry not created after incomplete op');
	}

	const txHashBuffer = decoded?.bdo?.tx;
	if (txHashBuffer) {
		const txEntry = await validatorPeer.base.view.get(txHashBuffer.toString('hex'));
		t.is(txEntry, null, 'tx hash not recorded after incomplete op');
	}
}

export default function bootstrapDeploymentIncompleteOperationScenario() {
	new OperationValidationScenarioBase({
		title: 'State.apply bootstrapDeployment rejects operations missing validator cosignature',
		setupScenario: setupBootstrapDeploymentScenario,
		buildValidPayload: buildBootstrapDeploymentPayload,
		mutatePayload: removeValidatorFields,
		applyInvalidPayload: async (context, invalidPayload) => {
			const { bootstrap } = context;
			await bootstrap.base.append(invalidPayload);
			await bootstrap.base.update();
		},
		assertStateUnchanged: (t, context, _validPayload, invalidPayload) =>
			assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload }),
		expectedLogs: ['Contract schema validation failed.']
	}).performScenario();
}
