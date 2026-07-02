import test from 'brittle';
import b4a from 'b4a';
import { decimalStringToBigInt, bigIntTo16ByteBuffer, bufferToBigInt, bigIntToDecimalString, licenseBufferToBigInt } from '../../../../src/utils/amountSerialization.js';
import { errorMessageIncludes } from "../../../helpers/regexHelper.js";
import lengthEntryUtils from '../../../../src/core/state/utils/lengthEntry.js';

test('decimalStringToBigInt', async t => {
    // Zero cases - all valid
    t.is(decimalStringToBigInt('0'), 0n, 'Simple zero');
    t.is(decimalStringToBigInt('0.0'), 0n, 'Zero with decimal point');
    t.is(decimalStringToBigInt('00.00'), 0n, 'Zero with leading and trailing zeros');

    // Valid cases
    t.is(decimalStringToBigInt('123'), 123000000000000000000n, 'Simple integer');
    t.is(decimalStringToBigInt('123.456'), 123456000000000000000n, 'Decimal number');
    t.is(decimalStringToBigInt('0.1'), 100000000000000000n, 'Decimal less than 1');
    t.is(decimalStringToBigInt('0.000000000000000001'), 1n, 'Smallest possible decimal');
    t.is(decimalStringToBigInt('1000000.000000000000000000'), 1000000000000000000000000n, 'Large number');
    t.is(decimalStringToBigInt('340282366920938463463.374607431768211455'), (2n ** 128n - 1n), 'Maximum allowed value');

    // Invalid cases
    await Promise.all([
        t.exception.all(() => decimalStringToBigInt(123),
            errorMessageIncludes('Input must be a string')),
        t.exception(() => decimalStringToBigInt('-1'),
            errorMessageIncludes('Negative amounts are not allowed')),
        t.exception(() => decimalStringToBigInt('-0.1'),
            errorMessageIncludes('Negative amounts are not allowed')),
        t.exception(() => decimalStringToBigInt('-123.456'),
            errorMessageIncludes('Negative amounts are not allowed')),
        t.exception(() => decimalStringToBigInt('-0'),
            errorMessageIncludes('Negative amounts are not allowed')),
        t.exception(() => decimalStringToBigInt('abc'),
            errorMessageIncludes('Invalid decimal format. Use format: 123.456')),
        t.exception(() => decimalStringToBigInt('123.456.789'),
            errorMessageIncludes('Invalid decimal format. Use format: 123.456')),
        t.exception(() => decimalStringToBigInt('123.1234567890123456789'),
            errorMessageIncludes('Too many decimal places. Maximum allowed: 18')),
        t.exception(() => decimalStringToBigInt(''),
            errorMessageIncludes('Invalid decimal format. Use format: 123.456')),
        t.exception(() => decimalStringToBigInt(' '),
            errorMessageIncludes('Invalid decimal format. Use format: 123.456')),
        t.exception(() => decimalStringToBigInt('12..34'),
            errorMessageIncludes('Invalid decimal format. Use format: 123.456')),
        t.exception(() => decimalStringToBigInt('.123'),
            errorMessageIncludes('Invalid decimal format. Use format: 123.456')),
        t.exception(() => decimalStringToBigInt('340282366920938463463.374607431768211456'),
            errorMessageIncludes('Amount exceeds maximum allowed value (2^128 - 1)')),
        t.exception(() => decimalStringToBigInt('999999999999999999999999999999999999999'),
            errorMessageIncludes('Amount exceeds maximum allowed value (2^128 - 1)'))
    ]);
});

test('bigIntTo16ByteBuffer', async t => {
    // Basic conversion tests
    const zeroResult = bigIntTo16ByteBuffer(0n);
    t.is(zeroResult.toString('hex'), '00000000000000000000000000000000', 'Should convert 0 to a buffer of all zeros');

    const oneResult = bigIntTo16ByteBuffer(1n);
    t.is(oneResult.toString('hex'), '00000000000000000000000000000001', 'Should convert 1 to a buffer of zeros with 1 at the end');

    const number256Result = bigIntTo16ByteBuffer(256n);
    t.is(number256Result.toString('hex'), '00000000000000000000000000000100', 'Should convert 256 to zeros with "0100" at the end (256 in hex)');

    const maxValueResult = bigIntTo16ByteBuffer(2n ** 128n - 1n);
    t.is(maxValueResult.toString('hex'), 'ffffffffffffffffffffffffffffffff', 'Should convert max value to all f\'s');

    const specificNumberResult = bigIntTo16ByteBuffer(123456789n);
    t.is(specificNumberResult.toString('hex'), '000000000000000000000000075bcd15', 'Should convert 123456789 to its proper hex representation');

    const overflowResult = bigIntTo16ByteBuffer(2n ** 128n + 5n);
    t.is(overflowResult.toString('hex'), '00000000000000000000000000000005', 'Number larger than 2^128 should wrap around to modulo');

    // Error cases tests
    await Promise.all([
        t.exception.all(() => bigIntTo16ByteBuffer(123),
            errorMessageIncludes('Input must be a BigInt')),
        t.exception.all(() => bigIntTo16ByteBuffer('123'),
            errorMessageIncludes('Input must be a BigInt')),
        t.exception.all(() => bigIntTo16ByteBuffer(null),
            errorMessageIncludes('Input must be a BigInt')),
        t.exception.all(() => bigIntTo16ByteBuffer(undefined),
            errorMessageIncludes('Input must be a BigInt'))
    ]);
});

test('bufferToBigInt', async t => {
    // Basic conversion tests
    const zeroBuffer = b4a.from('00000000000000000000000000000000', 'hex');
    t.is(bufferToBigInt(zeroBuffer), 0n, 'Should convert buffer of zeros to 0');

    const oneBuffer = b4a.from('00000000000000000000000000000001', 'hex');
    t.is(bufferToBigInt(oneBuffer), 1n, 'Should convert buffer with 1 at the end to 1');

    const number256Buffer = b4a.from('00000000000000000000000000000100', 'hex');
    t.is(bufferToBigInt(number256Buffer), 256n, 'Should convert buffer representing 256 (hex: 0100)');

    const maxValueBuffer = b4a.from('ffffffffffffffffffffffffffffffff', 'hex');
    t.is(bufferToBigInt(maxValueBuffer), (2n ** 128n - 1n), 'Should convert buffer of all f\'s to max value');

    const specificNumberBuffer = b4a.from('000000000000000000000000075bcd15', 'hex');
    t.is(bufferToBigInt(specificNumberBuffer), 123456789n, 'Should convert buffer to specific number (123456789)');

    // Roundtrip test - verify that conversion works both ways
    const originalValue = 987654321n;
    const buffer = bigIntTo16ByteBuffer(originalValue);
    const roundtripped = bufferToBigInt(buffer);
    t.is(roundtripped, originalValue, 'Should preserve value through buffer conversion roundtrip');

    // Error cases tests
    await Promise.all([
        t.exception.all(() => bufferToBigInt(b4a.alloc(15)),
            errorMessageIncludes('Input must be a 16-byte Buffer')),
        t.exception.all(() => bufferToBigInt(b4a.alloc(17)),
            errorMessageIncludes('Input must be a 16-byte Buffer')),
        t.exception.all(() => bufferToBigInt(null),
            errorMessageIncludes('Input must be a 16-byte Buffer')),
        t.exception.all(() => bufferToBigInt(undefined),
            errorMessageIncludes('Input must be a 16-byte Buffer')),
        t.exception.all(() => bufferToBigInt('not a buffer'),
            errorMessageIncludes('Input must be a 16-byte Buffer')),
        t.exception.all(() => bufferToBigInt([1, 2, 3, 4]),
            errorMessageIncludes('Input must be a 16-byte Buffer'))
    ]);
});

test('licenseBufferToBigInt', async t => {
    // Basic conversion tests
    const zeroBuffer = b4a.from('00000000', 'hex'); 
    t.is(licenseBufferToBigInt(zeroBuffer), 0n, 'Should convert buffer of zeros to 0');

    const oneBuffer = b4a.from('00000001', 'hex'); 
    t.is(licenseBufferToBigInt(oneBuffer), 1n, 'Should convert BE buffer with 1 at end to 1');

    const number256Buffer = b4a.from('00000100', 'hex'); 
    t.is(licenseBufferToBigInt(number256Buffer), 256n, 'Should convert BE buffer representing 256');

    const maxValueBuffer = b4a.from('ffffffff', 'hex'); 
    t.is(licenseBufferToBigInt(maxValueBuffer), (2n ** 32n - 1n), 'Should convert buffer of all f\'s to max 32-bit value');

    const specificNumberBuffer = b4a.from('075bcd15', 'hex');
    t.is(licenseBufferToBigInt(specificNumberBuffer), 123456789n, 'Should convert BE buffer to specific number (123456789)');

    // Roundtrip test
    const originalValue = 987654321n;
    const buffer = lengthEntryUtils.encodeBE(Number(originalValue));
    const roundtripped = licenseBufferToBigInt(buffer);
    t.is(roundtripped, originalValue, 'Should preserve value through buffer conversion roundtrip');

    // Error cases tests
    await Promise.all([
        t.exception.all(() => licenseBufferToBigInt(b4a.alloc(15)),
            errorMessageIncludes('Input must be a 4-byte Buffer')),
        t.exception.all(() => licenseBufferToBigInt(b4a.alloc(17)),
            errorMessageIncludes('Input must be a 4-byte Buffer')),
        t.exception.all(() => licenseBufferToBigInt(null),
            errorMessageIncludes('Input must be a 4-byte Buffer')),
        t.exception.all(() => licenseBufferToBigInt(undefined),
            errorMessageIncludes('Input must be a 4-byte Buffer')),
        t.exception.all(() => licenseBufferToBigInt('not a buffer'),
            errorMessageIncludes('Input must be a 4-byte Buffer')),
        t.exception.all(() => licenseBufferToBigInt([1, 2, 3, 4]),
            errorMessageIncludes('Input must be a 4-byte Buffer'))
    ]);
})

test('Integration: amount serialization roundtrip', async t => {
    // Test regular integer value
    const regularInt = '1';
    const regularIntBigInt = decimalStringToBigInt(regularInt);
    const regularIntBuffer = bigIntTo16ByteBuffer(regularIntBigInt);
    const regularIntRoundtrip = bufferToBigInt(regularIntBuffer);
    t.is(regularIntRoundtrip, regularIntBigInt, 'Should preserve regular integer through conversion');

    // Test decimal value
    const decimalValue = '123.456';
    const decimalBigInt = decimalStringToBigInt(decimalValue);
    const decimalBuffer = bigIntTo16ByteBuffer(decimalBigInt);
    const decimalRoundtrip = bufferToBigInt(decimalBuffer);
    t.is(decimalRoundtrip, decimalBigInt, 'Should preserve decimal value through conversion');

    // Test small decimal value
    const smallDecimal = '0.1';
    const smallDecimalBigInt = decimalStringToBigInt(smallDecimal);
    const smallDecimalBuffer = bigIntTo16ByteBuffer(smallDecimalBigInt);
    const smallDecimalRoundtrip = bufferToBigInt(smallDecimalBuffer);
    t.is(smallDecimalRoundtrip, smallDecimalBigInt, 'Should preserve small decimal through conversion');

    // Test large value with many decimal places
    const largeValue = '9999999.999999999999999999';
    const largeValueBigInt = decimalStringToBigInt(largeValue);
    const largeValueBuffer = bigIntTo16ByteBuffer(largeValueBigInt);
    const largeValueRoundtrip = bufferToBigInt(largeValueBuffer);
    t.is(largeValueRoundtrip, largeValueBigInt, 'Should preserve large value with many decimals through conversion');

    // Test smallest possible decimal
    const smallestValue = '0.000000000000000001';
    const smallestValueBigInt = decimalStringToBigInt(smallestValue);
    const smallestValueBuffer = bigIntTo16ByteBuffer(smallestValueBigInt);
    const smallestValueRoundtrip = bufferToBigInt(smallestValueBuffer);
    t.is(smallestValueRoundtrip, smallestValueBigInt, 'Should preserve smallest possible decimal through conversion');

    // Test maximum allowed value
    const maxValue = '340282366920938463463.374607431768211455';
    const maxValueBigInt = decimalStringToBigInt(maxValue);
    const maxValueBuffer = bigIntTo16ByteBuffer(maxValueBigInt);
    const maxValueRoundtrip = bufferToBigInt(maxValueBuffer);
    t.is(maxValueRoundtrip, maxValueBigInt, 'Should preserve maximum allowed value through conversion');

    // Verify that zero is handled correctly
    const zeroBigInt = decimalStringToBigInt('0');
    const zeroBuffer = bigIntTo16ByteBuffer(zeroBigInt);
    const zeroRoundtrip = bufferToBigInt(zeroBuffer);
    t.is(zeroRoundtrip, 0n, 'Should preserve zero through conversion');
});

test('bigIntToDecimalString', async t => {
    t.is(bigIntToDecimalString(1234567890000000000000000000n), '1234567890');
    t.is(bigIntToDecimalString(123456789012345678901234567890n), '123456789012.34567890123456789');
    t.is(bigIntToDecimalString(1n), '0.000000000000000001');
    t.is(bigIntToDecimalString(0n), '0');
    t.is(bigIntToDecimalString(1000000000000000000n), '1');
    t.is(bigIntToDecimalString(1000000000000000001n), '1.000000000000000001');

    await Promise.all([
        t.exception.all(() => bigIntToDecimalString(-1n),
            errorMessageIncludes('Negative amounts are not allowed')),
        t.exception.all(() => bigIntToDecimalString(2n ** 128n),
            errorMessageIncludes('Amount exceeds maximum allowed value')),
        t.exception.all(() => bigIntToDecimalString(1n, -1),
            errorMessageIncludes('Decimals must be a non-negative integer')),
        t.exception.all(() => bigIntToDecimalString(1n, 18.5),
            errorMessageIncludes('Decimals must be a non-negative integer')),
        t.exception.all(() => bigIntToDecimalString('123n'),
            errorMessageIncludes('Input must be a BigInt'))
    ]);
});
