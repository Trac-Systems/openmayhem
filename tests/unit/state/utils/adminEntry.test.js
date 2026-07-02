import { test } from 'brittle';
import b4a from 'b4a';
import { WRITER_BYTE_LENGTH } from '../../../../src/utils/constants.js';
import { randomAddress, randomBuffer } from '../stateTestUtils.js';
import addressUtils from '../../../../src/core/state/utils/address.js';
import adminEntryUtils from '../../../../src/core/state/utils/adminEntry.js';
import { config } from '../../../helpers/config.js';

const isAddressValid = address => addressUtils.isAddressValid(address, config.addressPrefix);
const addressToBuffer = addressUtils.addressToBuffer;
const encodeAdminEntry = (address, wk) => adminEntryUtils.encode(address, wk, config.addressPrefix);
const decodeAdminEntry = entry => adminEntryUtils.decode(entry, config.addressPrefix);
const ADMIN_ENTRY_SIZE = config.addressLength + WRITER_BYTE_LENGTH;

test('Admin Entry - Encode and Decode - Happy Path', t => {
    const address = randomAddress(config.addressPrefix);
    const wk = randomBuffer(WRITER_BYTE_LENGTH);

    const encoded = encodeAdminEntry(addressToBuffer(address, config.addressPrefix), wk);
    t.is(encoded.length, ADMIN_ENTRY_SIZE, "encoding has valid length");

    const decoded = decodeAdminEntry(encoded);
    t.ok(isAddressValid(address), 'Original address should be valid');
    t.ok(decoded, 'decoded should not be null');
    t.ok(decoded.address === address, 'address matches');
    t.ok(b4a.equals(decoded.wk, wk), 'wk matches');
});

test('Admin Entry - Encode returns empty buffer on invalid input', t => {
    const addrString = randomAddress(config.addressPrefix);
    const validAddress = addressToBuffer(addrString, config.addressPrefix);
    const separatorIndex = addrString.indexOf('1');
    const invalidAddress = validAddress.subarray(separatorIndex); // missing HRP

    const validWk = randomBuffer(WRITER_BYTE_LENGTH);
    const invalidWk = randomBuffer(10);

    const encoded1 = encodeAdminEntry(validAddress, invalidWk);
    const encoded2 = encodeAdminEntry(invalidAddress, validWk);

    console.log('Encoded1:', encoded1);
    console.log('Encoded2:', encoded2);

    t.is(encoded1.length, 0);
    t.is(encoded2.length, 0);
});

test('Admin Entry - Decode returns null on invalid input', t => {
    const buf = randomBuffer(10); // invalid size
    const decoded = decodeAdminEntry(buf);
    t.is(decoded, null);
});