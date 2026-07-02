import test from 'brittle';
import { isHexString } from '../../../../src/utils/helpers.js';

test('isHexString', (t) => {
    t.ok(isHexString('1234567890abcdef'), 'Valid hex string should return true with all lowercase characters');
    t.ok(isHexString('1234567890ABCDEF'), 'Valid hex string should return true with all uppercase characters');
    t.ok(isHexString('1234567890AbCdEf'), 'Valid hex string should return true with mixed case characters');
    t.is(isHexString('0x1234567890abcdef'), false, '0x prefixed hex string should return false');
    t.is(isHexString('1234567890xyz'), false, 'Invalid hex string should return false');
    t.is(isHexString('123456789'), false, 'Invalid size hex string should return false');
    t.is(isHexString(''), false, 'Empty string should return false');
});