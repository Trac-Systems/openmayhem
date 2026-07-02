import test from 'brittle';
import b4a from 'b4a';
import lengthEntry from '../../../../src/core/state/utils/lengthEntry.js';

// Test init()
test('Length Entry - init returns a 4-byte buffer initialized to 0', t => {
    const buf = lengthEntry.init();
    t.ok(b4a.isBuffer(buf), 'should return a buffer');
    t.is(buf.length, 4, 'buffer should be 4 bytes');
    t.alike(buf, b4a.alloc(4, 0x00), 'buffer should be all zeros');
});

// Test encode()
test('Length Entry - encode encodes integer to 4-byte buffer (Little Endian)', t => {
    const value = 123456;
    const buf = lengthEntry.encode(value);
    t.ok(b4a.isBuffer(buf), 'should return a buffer');
    t.is(buf.length, 4, 'buffer should be 4 bytes');
    t.is(buf.readUInt32LE(), value, 'buffer should decode to original value');
});

test('Length Entry - encode(0) returns buffer of all zeros', t => {
    const buf = lengthEntry.encode(0);
    t.alike(buf, b4a.alloc(4, 0x00));
});

// Test decode()
test('Length Entry - decode decodes 4-byte buffer to integer (LE)', t => {
    const value = 987654;
    const buf = b4a.alloc(4);
    buf.writeUInt32LE(value);
    t.is(lengthEntry.decode(buf), value);
});

// TODO: This test was deactivated because, currently, all function in lengthEntry.js can throw.
// When this behaviour is corrected, we should reactivate this test
// TODO: Also, implement other tests like this one for other functions in lengthEntry.js

// test('Length Entry - decode doesn\'t throw if buffer is too short', t => {
//     const shortBuf = b4a.alloc(2);
//     t.is(lengthEntry.decode(shortBuf), 0);
// });

// Test increment()
test('Length Entry - increment returns buffer with value+1', t => {
    const value = 42;
    const buf = lengthEntry.increment(value);
    t.is(buf.readUInt32LE(), value + 1);
});

test('Length Entry - increment(0) returns buffer with value 1', t => {
    const buf = lengthEntry.increment(0);
    t.is(buf.readUInt32LE(), 1);
});
