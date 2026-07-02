import { test } from 'brittle';
import b4a from 'b4a';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import {
	setupBootstrapDeploymentScenario,
	buildBootstrapDeploymentPayload,
	assertBootstrapDeploymentSuccessState
} from './bootstrapDeploymentScenarioHelpers.js';
import { EntryType } from '../../../../../src/utils/constants.js';
import { decimalStringToBigInt, bigIntTo16ByteBuffer } from '../../../../../src/utils/amountSerialization.js';

const DOUBLE_FUNDING = bigIntTo16ByteBuffer(decimalStringToBigInt('20'));

export default function bootstrapDeploymentMultipleBootstrapScenario() {
	test(
		'State.apply bootstrapDeployment allows a requester to register multiple external bootstraps',
		async t => {
			const context = await setupBootstrapDeploymentScenario(t, {
				nodes: 4,
				deployerInitialBalance: DOUBLE_FUNDING,
				validatorInitialBalance: DOUBLE_FUNDING
			});

			const validatorPeer = context.bootstrapDeployment?.validatorPeer ?? context.peers[1];
			const deployerPeer = context.bootstrapDeployment?.deployerPeer ?? context.peers[2];
			const extraPeer = context.peers.at(-1);

			const firstBootstrap = context.bootstrapDeployment?.externalBootstrap ?? b4a.from(deployerPeer.base.local.key);
			const secondBootstrap = extraPeer ? b4a.from(extraPeer.base.local.key) : b4a.alloc(firstBootstrap.length, 0x02);
			const secondChannel = extraPeer ? b4a.from(extraPeer.base.local.key) : b4a.alloc(firstBootstrap.length, 0x03);

			const validatorEntryBefore1 = await validatorPeer.base.view.get(validatorPeer.wallet.address);
			const deployerEntryBefore1 = await validatorPeer.base.view.get(deployerPeer.wallet.address);

			const firstPayload = await buildBootstrapDeploymentPayload(context, {
				externalBootstrap: firstBootstrap
			});

			await validatorPeer.base.append(firstPayload);
			await validatorPeer.base.update();
			await eventFlush();

			await assertBootstrapDeploymentSuccessState(t, context, {
				payload: firstPayload,
				externalBootstrap: firstBootstrap,
				validatorEntryBefore: validatorEntryBefore1?.value,
				deployerEntryBefore: deployerEntryBefore1?.value
			});

			const validatorEntryBefore2 = await validatorPeer.base.view.get(validatorPeer.wallet.address);
			const deployerEntryBefore2 = await validatorPeer.base.view.get(deployerPeer.wallet.address);

			const secondPayload = await buildBootstrapDeploymentPayload(context, {
				externalBootstrap: secondBootstrap,
				channel: secondChannel
			});

			await validatorPeer.base.append(secondPayload);
			await validatorPeer.base.update();
			await eventFlush();

			await assertBootstrapDeploymentSuccessState(t, context, {
				payload: secondPayload,
				externalBootstrap: secondBootstrap,
				channel: secondChannel,
				validatorEntryBefore: validatorEntryBefore2?.value,
				deployerEntryBefore: deployerEntryBefore2?.value
			});

			const firstKey = `${EntryType.DEPLOYMENT}${firstBootstrap.toString('hex')}`;
			const secondKey = `${EntryType.DEPLOYMENT}${secondBootstrap.toString('hex')}`;

			const firstEntry = await validatorPeer.base.view.get(firstKey);
			const secondEntry = await validatorPeer.base.view.get(secondKey);
			t.ok(firstEntry, 'first deployment entry stored on validator view');
			t.ok(secondEntry, 'second deployment entry stored on validator view');

			await context.sync();

			const firstEntryReplica = await deployerPeer.base.view.get(firstKey);
			const secondEntryReplica = await deployerPeer.base.view.get(secondKey);
			t.ok(firstEntryReplica, 'first deployment entry replicated to deployer view');
			t.ok(secondEntryReplica, 'second deployment entry replicated to deployer view');
		}
	);
}
