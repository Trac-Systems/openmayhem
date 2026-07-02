import { test } from 'brittle';
import b4a from 'b4a';
import { WRITER_BYTE_LENGTH, LICENSE_BYTE_LENGTH } from '../../../../src/utils/constants.js';
import { randomBuffer, TEN_THOUSAND_VALUE } from '../stateTestUtils.js';
import { NodeRole } from '../../../../src/core/state/utils/roles.js';
import {
    ZERO_BALANCE,
    ZERO_LICENSE,
    NODE_ENTRY_SIZE,
    init as initNodeEntry,
    encode as encodeNodeEntry,
    decode as decodeNodeEntry,
    setRole as setNodeEntryRole,
    setBalance as setNodeEntryBalance,
    setLicense as setNodeEntryLicense,
    setStakedBalance as setNodeEntryStakedBalance,
} from '../../../../src/core/state/utils/nodeEntry.js';

// Test init()
test('Node Entry - init - Happy Path', t => {
    const wk = randomBuffer(WRITER_BYTE_LENGTH)
    const initialized = initNodeEntry(wk, NodeRole.WHITELISTED)

    t.is(initialized.length, NODE_ENTRY_SIZE, "initialized has valid length");
    const decoded = decodeNodeEntry(initialized);
    t.ok(decoded, 'decoded should not be null');
    t.ok(b4a.equals(decoded.wk, wk), 'wk matches');
    t.is(decoded.isWhitelisted, true, 'isWhitelisted matches');
    t.is(decoded.isWriter, false, 'isWriter matches');
    t.is(decoded.isIndexer, false, 'isIndexer matches');
    t.ok(b4a.equals(decoded.balance, ZERO_BALANCE), 'balance matches');
    t.ok(b4a.equals(decoded.license, ZERO_LICENSE), 'license matches');
    t.ok(b4a.equals(decoded.stakedBalance, ZERO_BALANCE), 'stakedBalance matches');
});

test('Node Entry - init - Happy Path with balance', t => {
    const wk = randomBuffer(WRITER_BYTE_LENGTH)
    const initialized = initNodeEntry(wk, NodeRole.WHITELISTED, TEN_THOUSAND_VALUE)

    t.is(initialized.length, NODE_ENTRY_SIZE, "initialized has valid length");
    const decoded = decodeNodeEntry(initialized);
    t.ok(decoded, 'decoded should not be null');
    t.ok(b4a.equals(decoded.wk, wk), 'wk matches');
    t.is(decoded.isWhitelisted, true, 'isWhitelisted matches');
    t.is(decoded.isWriter, false, 'isWriter matches');
    t.is(decoded.isIndexer, false, 'isIndexer matches');
    t.ok(b4a.equals(decoded.balance, TEN_THOUSAND_VALUE), 'balance matches');
});

test('Node Entry - init - Happy Path with licesnse', t => {
    const wk = randomBuffer(WRITER_BYTE_LENGTH)
    const license = randomBuffer(LICENSE_BYTE_LENGTH)
    const initialized = initNodeEntry(wk, NodeRole.WHITELISTED, TEN_THOUSAND_VALUE, license)

    t.is(initialized.length, NODE_ENTRY_SIZE, "initialized has valid length");
    const decoded = decodeNodeEntry(initialized);
    t.ok(decoded, 'decoded should not be null');
    t.ok(b4a.equals(decoded.wk, wk), 'wk matches');
    t.is(decoded.isWhitelisted, true, 'isWhitelisted matches');
    t.is(decoded.isWriter, false, 'isWriter matches');
    t.is(decoded.isIndexer, false, 'isIndexer matches');
    t.ok(b4a.equals(decoded.balance, TEN_THOUSAND_VALUE), 'balance matches');
    t.ok(b4a.equals(decoded.license, license), 'license matches');
});

test('Node Entry - init - Happy Path with staked balance', t => {
    const wk = randomBuffer(WRITER_BYTE_LENGTH)
    const license = randomBuffer(LICENSE_BYTE_LENGTH)
    const initialized = initNodeEntry(wk, NodeRole.WHITELISTED, TEN_THOUSAND_VALUE, license, TEN_THOUSAND_VALUE)

    t.is(initialized.length, NODE_ENTRY_SIZE, "initialized has valid length");
    const decoded = decodeNodeEntry(initialized);
    t.ok(decoded, 'decoded should not be null');
    t.ok(b4a.equals(decoded.wk, wk), 'wk matches');
    t.is(decoded.isWhitelisted, true, 'isWhitelisted matches');
    t.is(decoded.isWriter, false, 'isWriter matches');
    t.is(decoded.isIndexer, false, 'isIndexer matches');
    t.ok(b4a.equals(decoded.balance, TEN_THOUSAND_VALUE), 'balance matches');
    t.ok(b4a.equals(decoded.license, license), 'license matches');
    t.ok(b4a.equals(decoded.stakedBalance, TEN_THOUSAND_VALUE), 'stakedBalance matches');

});

// Test encode()
test('Node Entry - encode and decode - Happy Path', t => {
    const node = {
        wk: randomBuffer(WRITER_BYTE_LENGTH),
        isWhitelisted: true,
        isWriter: true,
        isIndexer: false,
        balance: ZERO_BALANCE,
        license: ZERO_LICENSE,
        stakedBalance: ZERO_BALANCE
    };

    const encoded = encodeNodeEntry(node);
    t.is(encoded.length, NODE_ENTRY_SIZE, "encoding has valid length");

    const decoded = decodeNodeEntry(encoded);
    t.ok(decoded, 'decoded should not be null');
    t.ok(b4a.equals(decoded.wk, node.wk), 'wk matches');
    t.is(decoded.isWhitelisted, true, 'isWhitelisted matches');
    t.is(decoded.isWriter, true, 'isWriter matches');
    t.is(decoded.isIndexer, false, 'isIndexer matches');
    t.ok(b4a.equals(decoded.balance, ZERO_BALANCE), 'balance matches');
    t.ok(b4a.equals(decoded.license, ZERO_LICENSE), 'license matches');
    t.ok(b4a.equals(decoded.stakedBalance, ZERO_BALANCE), 'stakedBalance matches');
});

test('Node Entry - encode returns empty buffer on invalid wk', t => {
    const node = {
        wk: randomBuffer(10), // invalid size
        isWhitelisted: false,
        isWriter: false,
        isIndexer: false,
        balance: ZERO_BALANCE,
        license: ZERO_LICENSE,
        stakedBalance: ZERO_BALANCE

    };
    const encoded = encodeNodeEntry(node);
    t.is(encoded.length, 0);
});

test('Node Entry - encode returns empty buffer on invalid node role', t => {
    const node = {
        wk: randomBuffer(WRITER_BYTE_LENGTH),
        isWhitelisted: false,
        isWriter: true, // can't be a writer without being whitelisted
        isIndexer: false,
        balance: ZERO_BALANCE
    };
    const encoded = encodeNodeEntry(node);
    t.is(encoded.length, 0);
});

test('Node Entry - encode returns empty buffer on invalid balance', t => {
    const node = {
        wk: randomBuffer(WRITER_BYTE_LENGTH),
        isWhitelisted: true,
        isWriter: true,
        isIndexer: false,
        balance: b4a.from([
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x27,
            0x10,
        ])
    };
    const encoded = encodeNodeEntry(node);
    t.is(encoded.length, 0);
});

test('Node Entry - encode returns empty buffer on invalid license', t => {
    const node = {
        wk: randomBuffer(WRITER_BYTE_LENGTH),
        isWhitelisted: true,
        isWriter: true,
        isIndexer: false,
        balance: ZERO_BALANCE,
        license: randomBuffer(10), // invalid size
        stakedBalance: ZERO_BALANCE

    };
    const encoded = encodeNodeEntry(node);
    t.is(encoded.length, 0);
});

test('Node Entry - encode returns empty buffer on invalid staked balance', t => {
    const node = {
        wk: randomBuffer(WRITER_BYTE_LENGTH),
        isWhitelisted: true,
        isWriter: true,
        isIndexer: false,
        balance: ZERO_BALANCE,
        license: ZERO_LICENSE,
        stakedBalance: b4a.from([
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x27,
            0x10,
        ])
    };
    const encoded = encodeNodeEntry(node);
    t.is(encoded.length, 0);
});

// Test decode()
test('Node Entry - decode returns null on invalid input', t => {
    const buf = randomBuffer(10); // invalid size
    const decoded = decodeNodeEntry(buf);
    t.is(decoded, null);
});

test('Node Entry - setRole updates role flags', t => {
    const TEN_THOUSAND_VALUE = b4a.from([
        0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x27, 0x10,
    ])

    let entry = encodeNodeEntry({
        wk: randomBuffer(WRITER_BYTE_LENGTH),
        isWhitelisted: false,
        isWriter: false,
        isIndexer: false,
        balance: TEN_THOUSAND_VALUE,
        license: ZERO_LICENSE,
        stakedBalance: ZERO_BALANCE
    });

    // Set maximum tier role (0x7)
    entry = setNodeEntryRole(entry, NodeRole.INDEXER);
    const decoded = decodeNodeEntry(entry);
    t.ok(decoded.isWhitelisted, 'isWhitelisted should be true');
    t.ok(decoded.isWriter, 'isWriter should be true');
    t.ok(decoded.isIndexer, 'isIndexer should be true');
    t.ok(b4a.equals(decoded.balance, TEN_THOUSAND_VALUE), 'balance should not change');

    // Set to no roles (0x0)
    entry = setNodeEntryRole(entry, NodeRole.READER);
    const decoded2 = decodeNodeEntry(entry);
    t.not(decoded2.isWhitelisted, 'isWhitelisted should be false');
    t.not(decoded2.isWriter, 'isWriter should be false');
    t.not(decoded2.isIndexer, 'isIndexer should be false');
    t.ok(b4a.equals(decoded.balance, TEN_THOUSAND_VALUE), 'balance should not change');
});

test('Node Entry - setBalance updates the balance', t => {
    let entry = encodeNodeEntry({
        wk: randomBuffer(WRITER_BYTE_LENGTH),
        isWhitelisted: false,
        isWriter: false,
        isIndexer: false,
        balance: ZERO_BALANCE,
        license: ZERO_LICENSE,
        stakedBalance: ZERO_BALANCE
    });

    entry = setNodeEntryBalance(entry, TEN_THOUSAND_VALUE);
    const decoded = decodeNodeEntry(entry);
    t.not(decoded.isWhitelisted, 'isWhitelisted should be true');
    t.not(decoded.isWriter, 'isWriter should be true');
    t.not(decoded.isIndexer, 'isIndexer should be true');
    t.ok(b4a.equals(decoded.balance, TEN_THOUSAND_VALUE), 'balance should be 10k');

    const INVALID_BALANCE = b4a.from([
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x27,
        0x10,
    ])

    // Set to no roles (0x0)
    entry = setNodeEntryBalance(entry, INVALID_BALANCE);
    const decoded2 = decodeNodeEntry(entry);
    t.is(decoded2, null);
});

test('Node Entry - setLicense updates the license', t => {
    let entry = encodeNodeEntry({
        wk: randomBuffer(WRITER_BYTE_LENGTH),
        isWhitelisted: false,
        isWriter: false,
        isIndexer: false,
        balance: ZERO_BALANCE,
        license: ZERO_LICENSE,
        stakedBalance: ZERO_BALANCE
    });
    const randomLicense = randomBuffer(LICENSE_BYTE_LENGTH);
    entry = setNodeEntryLicense(entry, randomLicense);
    const decoded = decodeNodeEntry(entry);
    t.not(decoded.isWhitelisted, 'isWhitelisted should be true');
    t.not(decoded.isWriter, 'isWriter should be true');
    t.not(decoded.isIndexer, 'isIndexer should be true');
    t.ok(b4a.equals(decoded.balance, ZERO_BALANCE), 'balance should be 0');
    t.ok(b4a.equals(decoded.license, randomLicense), 'license should be set');
});

// set staked balance

test('Node Entry - setStakedBalance updates the staked balance', t => {
    let entry = encodeNodeEntry({
        wk: randomBuffer(WRITER_BYTE_LENGTH),
        isWhitelisted: false,
        isWriter: false,
        isIndexer: false,
        balance: ZERO_BALANCE,
        license: ZERO_LICENSE,
        stakedBalance: ZERO_BALANCE
    });
    entry = setNodeEntryStakedBalance(entry, TEN_THOUSAND_VALUE);
    const decoded = decodeNodeEntry(entry);
    t.not(decoded.isWhitelisted, 'isWhitelisted should be true');
    t.not(decoded.isWriter, 'isWriter should be true');
    t.not(decoded.isIndexer, 'isIndexer should be true');
    t.ok(b4a.equals(decoded.balance, ZERO_BALANCE), 'balance should be 0');
    t.ok(b4a.equals(decoded.license, ZERO_LICENSE), 'license should be set');
    t.ok(b4a.equals(decoded.stakedBalance, TEN_THOUSAND_VALUE), 'staked balance should be 10k');
});
