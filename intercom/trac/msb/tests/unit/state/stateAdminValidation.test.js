import { test } from 'brittle';
import sinon from 'sinon';
import b4a from 'b4a';
import { address as addressApi } from 'trac-crypto-api';
import { randomAddress, randomBuffer } from './stateTestUtils.js';
import { WRITER_BYTE_LENGTH } from '../../../src/utils/constants.js';
import addressUtils from '../../../src/core/state/utils/address.js';

import fs from 'fs';
import Corestore from 'corestore';
import State from '../../../src/core/state/State.js';

function createStateConfig(overrides = {}) {
    return {
        addressPrefix: 'trac',
        addressLength: addressApi.size('trac'),
        bootstrap: randomBuffer(WRITER_BYTE_LENGTH),
        maxWritersForAdminIndexerConnection: 2,
        ...overrides
    };
}

async function setupMockedState(config) {
    const dbPath = './.test-db-' + Date.now() + Math.floor(Math.random() * 1000);
    const store = new Corestore(dbPath);
    const dummyWallet = {};

    const state = new State(store, dummyWallet, config);

    if (typeof state._open === 'function') await state._open();
    else if (typeof state.ready === 'function') await state.ready();

    const stubs = [];
    
    stubs.push(sinon.stub(state, 'writingKey').get(() => config.bootstrap));
    stubs.push(sinon.stub(state, 'isIndexer').resolves(true));

    const adminAddress = randomAddress('trac');
    const adminAddressBuffer = addressUtils.addressToBuffer(adminAddress, 'trac');
    stubs.push(sinon.stub(state, 'getAdminEntry').resolves({ address: adminAddress, wk: config.bootstrap }));

    const writerOneAddress = randomAddress('trac');
    const writerOneAddressBuffer = addressUtils.addressToBuffer(writerOneAddress, 'trac');

    const adminKey = config.bootstrap;
    const writerKeyActive = randomBuffer(WRITER_BYTE_LENGTH);
    const writerKeyRemoved = randomBuffer(WRITER_BYTE_LENGTH);

    stubs.push(sinon.stub(state, 'getRegisteredWriterKey').callsFake(async (input) => {
        if (!input) return null;
        const hex = b4a.isBuffer(input) ? b4a.toString(input, 'hex') : input;
        
        if (hex === adminKey.toString('hex')) return adminAddressBuffer;
        if (hex === writerKeyActive.toString('hex')) return writerOneAddressBuffer;
        if (hex === writerKeyRemoved.toString('hex')) return writerOneAddressBuffer;
        return null;
    }));

    stubs.push(sinon.stub(state, 'getNodeEntry').callsFake(async (input) => {
        if (!input) return null;
        const hex = b4a.isBuffer(input) ? b4a.toString(input, 'hex') : input;
        
        if (hex === adminKey.toString('hex')) return { isRemoved: false };
        if (hex === writerKeyActive.toString('hex')) return { isRemoved: false };
        if (hex === writerKeyRemoved.toString('hex')) return { isRemoved: true };
        return null;
    }));

    const listEntries = [
        { key: adminKey, value: { isIndexer: true, isRemoved: false } },
        { key: writerKeyActive, value: { isIndexer: false, isRemoved: false } }, 
        { key: writerKeyRemoved, value: { isIndexer: false, isRemoved: true } }
    ];

    if (!state.base) state.base = {};
    if (!state.base.system) state.base.system = {};

    stubs.push(sinon.stub(state.base.system, 'list').callsFake(async function* () {
        for (const entry of listEntries) {
            yield entry;
        }
    }));

    return { 
        state, 
        restore: () => {
            stubs.forEach(s => s.restore());
            try { fs.rmSync(dbPath, { recursive: true, force: true }); } catch (e) {
                console.log(e);
            }
        } 
    };
}

test('State#getActiveWriterCount deduplicates active writers and ignores removed/non-admin indexers', async t => {
    const config = createStateConfig();
    const { state, restore } = await setupMockedState(config);

    const countTotal = await state.getActiveWriterCount();
    t.is(countTotal, 2, 'counts admin plus one active validator');

    const countWithoutAdmin = await state.getActiveWriterCount(true);
    t.is(countWithoutAdmin, 1, 'counts only active non-admin validators');

    restore();
});

test('State#isAdminAllowedToValidate blocks admin when active validator threshold is reached', async t => {
    const config = createStateConfig({ maxWritersForAdminIndexerConnection: 1 });
    const { state, restore } = await setupMockedState(config);

    const allowed = await state.isAdminAllowedToValidate();
    t.is(allowed, false, 'admin is blocked when active validator count reaches threshold');

    restore();
});
