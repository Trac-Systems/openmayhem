import { test } from 'brittle';
import b4a from 'b4a';
import { eventFlush } from '../../../../helpers/autobaseTestHelpers.js';
import nodeEntryUtils from '../../../../../src/core/state/utils/nodeEntry.js';
import { toTerm } from '../../../../../src/core/state/utils/balance.js';
import { ZERO_WK } from '../../../../../src/utils/buffer.js';
import { safeDecodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import setupBalanceInitializationScenario, {
	buildBalanceInitializationPayload
} from './balanceInitializationScenarioHelpers.js';

export default function balanceInitializationHappyPathScenario() {
	test('State.apply balanceInitialization credits reader nodes with requested balances - happy path', async t => {
		const recipientCount = 2;
		const networkContext = await setupBalanceInitializationScenario(t, recipientCount);
		const adminNode = networkContext.adminBootstrap;
		const readerNodes = networkContext.peers.slice(1);
		const recipients = [];

		for (let i = 0; i < recipientCount; i++) {
			const peer = readerNodes[i];
			recipients.push({
				peer,
				balance: i === 0 ? toTerm(25n) : toTerm(75n)
			});
		}

		const appliedTxHashes = [];

		for (const recipient of recipients) {
			const payload = await buildBalanceInitializationPayload(
				networkContext,
				recipient.peer.wallet.address,
				recipient.balance
			);
			const decodedPayload = safeDecodeApplyOperation(payload);
			appliedTxHashes.push(decodedPayload.bio.tx.toString('hex'));

			await adminNode.base.append(payload);
			await adminNode.base.update();
			await eventFlush();
		}

		for (const recipient of recipients) {
			await assertRecipientBalance(
				t,
				adminNode.base,
				recipient.peer.wallet.address,
				recipient.balance
			);
		}

		for (const txHashHex of appliedTxHashes) {
			const txEntry = await adminNode.base.view.get(txHashHex);
			t.ok(txEntry, 'operation hash stored to prevent replays');
		}

		await networkContext.sync();

		for (const recipient of recipients) {
			await assertRecipientBalance(
				t,
				recipient.peer.base,
				recipient.peer.wallet.address,
				recipient.balance
			);
		}
	});
}

async function assertRecipientBalance(t, base, address, expectedBalance) {
	const nodeEntryRecord = await base.view.get(address);
	t.ok(nodeEntryRecord, 'recipient node entry exists');

	const decodedEntry = nodeEntryUtils.decode(nodeEntryRecord.value);
	t.ok(decodedEntry, 'recipient node entry decodes');
	t.is(decodedEntry.isWriter, false, 'recipient not flagged as writer');
	t.is(decodedEntry.isIndexer, false, 'recipient not flagged as indexer');
	t.ok(b4a.equals(decodedEntry.wk, ZERO_WK), 'recipient writing key remains unset');
	t.ok(b4a.equals(decodedEntry.balance, expectedBalance), 'recipient balance is set');
	t.ok(b4a.equals(decodedEntry.stakedBalance, toTerm(0n)), 'recipient staked balance is zero');
}
