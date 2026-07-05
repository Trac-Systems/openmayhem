import { test } from 'brittle';

import {
	setupTransferScenario,
	buildBatchTransferPayload,
	snapshotTransferEntries,
	decodeEntryBalance
} from './transferScenarioHelpers.js';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import transactionUtils from '../../../../../src/core/state/utils/transaction.js';
import { decimalStringToBigInt, bufferToBigInt } from '../../../../../src/utils/amountSerialization.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';

export default function transferBatchAmountScenario() {
	test('State.apply batch transfer pays multiple recipients with one fee', async t => {
		const context = await setupTransferScenario(t, { nodes: 5, recipientHasEntry: true });
		const { senderPeer, recipientPeer, validatorPeer } = context.transferScenario;
		const recipientB = context.peers.slice(1)[3];
		t.ok(recipientB, 'second recipient available');

		const snapshots = await snapshotTransferEntries(context, { senderPeer, recipientPeer, validatorPeer });
		const recipientBBefore = await validatorPeer.base.view.get(recipientB.wallet.address);
		t.is(recipientBBefore, null, 'second recipient starts without an entry');

		const amountA = decimalStringToBigInt('1');
		const amountB = decimalStringToBigInt('0.5');
		const { payload, batch } = await buildBatchTransferPayload(context, {
			outputs: [
				{ to: recipientPeer.wallet.address, amount: '1' },
				{ to: recipientB.wallet.address, amount: '0.5' },
			]
		});

		await validatorPeer.base.append(payload);
		await validatorPeer.base.update();
		await eventFlush();
		await context.sync();

		const decodedPayload = safeDecodeApplyOperation(payload);
		t.ok(decodedPayload?.tro?.bo, 'batch transfer payload carries batch outputs');
		t.ok(decodedPayload?.tro?.ba, 'batch transfer payload carries batch total');
		t.is(bufferToBigInt(batch.totalAmount), amountA + amountB, 'batch total matches output sum');

		const senderBefore = decodeEntryBalance(snapshots.senderEntry.value);
		const recipientABefore = decodeEntryBalance(snapshots.recipientEntry.value);
		const validatorBefore = decodeEntryBalance(snapshots.validatorEntry.value);
		t.ok(senderBefore !== null, 'sender balance decoded before batch');
		t.ok(recipientABefore !== null, 'recipient A balance decoded before batch');
		t.ok(validatorBefore !== null, 'validator balance decoded before batch');
		if (senderBefore === null || recipientABefore === null || validatorBefore === null) return;

		const fee = bufferToBigInt(transactionUtils.FEE);
		const validatorReward = (fee * 7500n) / 10000n;

		const senderAfter = await validatorPeer.base.view.get(senderPeer.wallet.address);
		const recipientAAfter = await validatorPeer.base.view.get(recipientPeer.wallet.address);
		const recipientBAfter = await validatorPeer.base.view.get(recipientB.wallet.address);
		const validatorAfter = await validatorPeer.base.view.get(validatorPeer.wallet.address);

		t.ok(senderAfter?.value, 'sender entry exists after batch');
		t.ok(recipientAAfter?.value, 'recipient A entry exists after batch');
		t.ok(recipientBAfter?.value, 'recipient B entry exists after batch');
		t.ok(validatorAfter?.value, 'validator entry exists after batch');
		if (!senderAfter?.value || !recipientAAfter?.value || !recipientBAfter?.value || !validatorAfter?.value) return;

		t.is(decodeEntryBalance(senderAfter.value), senderBefore - amountA - amountB - fee, 'sender pays batch total plus one fee');
		t.is(decodeEntryBalance(recipientAAfter.value), recipientABefore + amountA, 'existing recipient credited');
		t.is(decodeEntryBalance(recipientBAfter.value), amountB, 'new recipient credited');
		t.is(decodeEntryBalance(validatorAfter.value), validatorBefore + validatorReward, 'validator gets one fee reward');

		const recipientBDecoded = nodeEntryUtils.decode(recipientBAfter.value);
		t.ok(recipientBDecoded, 'new recipient entry decodes');
		t.is(recipientBDecoded?.isWriter, false, 'new recipient is not a writer');
		t.is(recipientBDecoded?.isWhitelisted, false, 'new recipient is not whitelisted');
		t.is(recipientBDecoded?.isIndexer, false, 'new recipient is not an indexer');

		const txHash = decodedPayload?.tro?.tx?.toString('hex') ?? '';
		t.ok(txHash, 'batch tx hash available');
		const txEntry = txHash ? await validatorPeer.base.view.get(txHash) : null;
		t.ok(txEntry, 'batch transfer hash recorded for replay protection');
	});
}
