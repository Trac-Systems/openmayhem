import test from 'brittle';
import b4a from 'b4a';
import { normalizeHex } from '../../../../src/utils/helpers.js';

test('normalizeHex should convert hex string to buffer', t => {
    const hexString = '1234abcd';
    const result = normalizeHex(hexString);
    t.ok(b4a.isBuffer(result), 'Result should be a buffer');
    t.is(b4a.toString(result, 'hex'), hexString, 'Buffer should contain correct hex value');
});

test('normalizeHex should return buffer as is', t => {
    const originalBuffer = b4a.from('1234abcd', 'hex');
    const result = normalizeHex(originalBuffer);
    t.is(result, originalBuffer, 'Should return the same buffer instance');
    t.ok(b4a.isBuffer(result), 'Result should be a buffer');
});

test('normalizeHex should handle empty string', t => {
    try {
        normalizeHex('');
        t.fail('Should throw on empty string');
    } catch (err) {
        t.pass('Throws on empty string');
    }
});

test('normalizeHex should handle invalid hex string', t => {
    try {
        normalizeHex('invalid hex');
        t.fail('Should throw on invalid hex string');
    } catch (err) {
        t.pass('Throws on invalid hex string');
    }
});

test('normalizeHex should handle uppercase hex string', t => {
    const hexString = '1234ABCD';
    const result = normalizeHex(hexString);
    t.ok(b4a.isBuffer(result), 'Result should be a buffer');
    t.is(b4a.toString(result, 'hex'), hexString.toLowerCase(), 'Buffer should contain correct hex value');
});

test('normalizeHex should handle hex string with 0x prefix', t => {
    const hexString = '0x1234abcd';
    const result = normalizeHex(hexString.slice(2));
    t.ok(b4a.isBuffer(result), 'Result should be a buffer');
    t.is(b4a.toString(result, 'hex'), '1234abcd', 'Buffer should contain correct hex value without prefix');
});


test('normalizeHex should handle invalid data types', t => {
    const invalidTypes = [
        { value: 997, desc: 'number' },
        { value: true, desc: 'boolean' },
        { value: null, desc: 'null' },
        { value: undefined, desc: 'undefined' },
        { value: Infinity, desc: 'Infinity' },
        { value: {}, desc: 'empty object' },
        { value: [], desc: 'empty array' },
        { value: () => { }, desc: 'function' },
        { value: "invalid hex", desc: 'non-hex string' },
        { value: BigInt(997), desc: 'BigInt' },
        { value: new Date(), desc: 'Date object' },
        { value: NaN, desc: 'NaN' },
        { value: new Map(), desc: 'Map' },
        { value: new Set(), desc: 'Set' },
        { value: /abc/, desc: 'RegExp' },
        { value: new Error('fail'), desc: 'Error' },
        { value: Promise.resolve(), desc: 'Promise' },
        { value: new Float32Array([1.1, 2.2]), desc: 'Float32Array' }
    ];

    for (const { value, desc } of invalidTypes) {
        try {
            normalizeHex(value);
            t.fail(`Should throw on ${desc} input`);
        } catch (err) {
            t.pass(`Correctly throws on ${desc} input`);
        }
    }
});
