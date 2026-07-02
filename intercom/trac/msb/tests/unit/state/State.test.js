import { test, hook } from 'brittle';
import { encode, decode, ZERO_BALANCE } from '../../src/core/state/utils/nodeEntry.js';
import esmock from "esmock";
import sinon from "sinon";
import { randomBuffer, tokenUnits } from './stateTestUtils.js';
import { WRITER_BYTE_LENGTH, ADMIN_INITIAL_BALANCE } from '../../src/utils/constants.js';
import { $TNK, toBalance } from '../../src/core/state/utils/balance.js';
import b4a from 'b4a'

let state
hook('Initialize state', async () => {
    const checkout = sinon.stub().returns({ get: () => {
            return { value: encode({
                wk: randomBuffer(WRITER_BYTE_LENGTH),
                isWhitelisted: true,
                isWriter: true,
                isIndexer: true,
                balance: ADMIN_INITIAL_BALANCE })
            }
        }
    });

    const AutoBaseMock = sinon.stub().returns({ view: { checkout, core: { signedLength: 1 }} });
    const State = await esmock('../../src/core/state/State.js', {
        "autobase": AutoBaseMock
    });
    state = new State(null, null, null)
});

test('State#incrementBalance', () => {
    test('adds two balances and produce a node entry', async t => {
        const entry = await state.incrementBalance('adminAddress', $TNK(150n))
        t.is(toBalance(decode(entry).balance).asBigInt(), tokenUnits(1150n), 'balance matches');
    })

    test('do not add zero', async t => {
        const entry = await state.incrementBalance('adminAddress', $TNK(0n))
        t.is(entry, null, 'entry is null');
    })

    test('null entry', async t => {
        const AutoBaseMock = sinon.stub().returns({ view: { checkout: sinon.stub().returns({ get: () => ({ value: null }) }), core: { signedLength: 1 }} });
        const State = await esmock('../../src/core/state/State.js', {
            "autobase": AutoBaseMock
        });
        const entry = await new State(null, null, null).incrementBalance('adminAddress', $TNK(150n))
        t.is(entry, null, 'entry is null');
    })
});

test('State#derementBalance', async t => {
    test('subtract two balances and produce a node entry', async t => {
        const entry = await state.decrementBalance('adminAddress', $TNK(150n))
        t.is(toBalance(decode(entry).balance).asBigInt(), tokenUnits(850n), 'balance matches');
    })

    test('do not subtract zero', async t => {
        const entry = await state.decrementBalance('adminAddress', $TNK(0n))
        t.is(entry, null, 'entry is null');
    })

    test('do not produce negative balance', async t => {
        const entry = await state.decrementBalance('adminAddress', $TNK(1001n))
        t.is(entry, null, 'entry is null');
    })

    test('can zero balance', async t => {
        const entry = await state.decrementBalance('adminAddress', $TNK(1000n))
        t.ok(b4a.equals(decode(entry).balance, ZERO_BALANCE), 'balance matches');
    })

    test('null entry', async t => {
        const AutoBaseMock = sinon.stub().returns({ view: { checkout: sinon.stub().returns({ get: () => ({ value: null }) }), core: { signedLength: 1 }} });
        const State = await esmock('../../src/core/state/State.js', {
            "autobase": AutoBaseMock
        });
        const entry = await new State(null, null, null).decrementBalance('adminAddress', $TNK(150n))
        t.is(entry, null, 'entry is null');
    })
});