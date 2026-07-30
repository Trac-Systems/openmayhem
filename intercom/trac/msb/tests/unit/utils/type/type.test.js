import test from 'brittle';
import { isDefined } from '../../../../src/utils/type.js';

const assertIsDefined = (t, value, expected, message) => {
    try {
        t.is(isDefined(value), expected, message);
    } catch (err) {
        t.fail(`${message}. Threw: ${err.message}`);
    }
};

test('isDefined returns false for nullish and NaN', t => {
    assertIsDefined(t, null, false, 'null should be treated as not defined');
    assertIsDefined(t, void 0, false, 'undefined should be treated as not defined');
    assertIsDefined(t, NaN, false, 'NaN should be treated as not defined');
});

test('isDefined returns true for defined non-NaN values', t => {
    assertIsDefined(t, 0, true, '0 should be treated as defined');
    assertIsDefined(t, Infinity, true, 'Infinity should be treated as defined');
    assertIsDefined(t, '', true, 'empty string should be treated as defined');
    assertIsDefined(t, false, true, 'boolean false should be treated as defined');
    assertIsDefined(t, {}, true, 'object should be treated as defined');
    assertIsDefined(t, [], true, 'array should be treated as defined');
});
