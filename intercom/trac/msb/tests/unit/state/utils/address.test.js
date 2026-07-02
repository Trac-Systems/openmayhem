import { test } from 'brittle';
import b4a from 'b4a';
import { randomAddress } from '../stateTestUtils.js';
import addressUtils from '../../../../src/core/state/utils/address.js';
import { address as addressApi } from 'trac-crypto-api';

test('Convert bech32m address to and from buffer - Happy Path', t => {
    const hrp = 'test';
    const address = randomAddress(hrp);
    const addressBuffer = addressUtils.addressToBuffer(address, hrp);
    const reconstructedAddress = addressUtils.bufferToAddress(addressBuffer, hrp);

    t.ok(addressUtils.isAddressValid(address, hrp), 'Original address should be valid');
    t.ok(b4a.isBuffer(addressBuffer), 'Address buffer should be a Buffer instance');
    t.is(typeof reconstructedAddress, 'string', 'Reconstructed address should be a string');
    t.is(address, reconstructedAddress, 'Reconstructed address should match original');
    t.is(address.length, addressApi.size(hrp), 'Address length should match expected size');
    t.is(addressBuffer.length, addressApi.size(hrp), 'Address buffer length should match address length');
});

test('isAddressValid returns false for wrong prefix', t => {
    const hrp = 'test';
    const address = randomAddress(hrp);
    t.not(addressUtils.isAddressValid(address, 'wrong'), 'Should be invalid for wrong prefix');
});

test('isAddressValid returns false for wrong length', t => {
    const hrp = 'test';
    const address = randomAddress(hrp);
    const short = address.slice(0, -1);
    t.not(addressUtils.isAddressValid(short, hrp), 'Should be invalid for short address');
    const long = address + 'a';
    t.not(addressUtils.isAddressValid(long, hrp), 'Should be invalid for long address');
});

test('isAddressValid returns false for invalid characters', t => {
    const hrp = 'test';
    let address = randomAddress(hrp);
    // Replace a char with an invalid one
    address = address.slice(0, 6) + 'A' + address.slice(7);
    t.not(addressUtils.isAddressValid(address, hrp), 'Should be invalid for non-bech32 chars');
});

test('addressToBuffer returns empty buffer for invalid address', t => {
    const invalid = 'notanaddress';
    const buf = addressUtils.addressToBuffer(invalid, 'test');
    t.ok(b4a.isBuffer(buf));
    t.is(buf.length, 0, 'Should return empty buffer');
});

test('bufferToAddress returns null for invalid buffer', t => {
    const buf = b4a.alloc(10, 0x61); // too short
    t.is(addressUtils.bufferToAddress(buf, 'test'), null);
});

test('bufferToAddress returns null for buffer with invalid chars', t => {
    const hrp = 'test';
    const address = randomAddress(hrp);
    const buf = b4a.from(address, 'ascii');
    // Corrupt the buffer
    buf[5] = 0x41; // 'A' (not bech32)
    t.is(addressUtils.bufferToAddress(buf, hrp), null);
});
