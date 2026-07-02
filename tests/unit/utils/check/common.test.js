import b4a from 'b4a';

import { MIN_SAFE_VALIDATION_INTEGER, MAX_SAFE_VALIDATION_INTEGER } from '../../../../src/utils/constants.js';
import { partial_operation_value_type } from "../../../fixtures/check.fixtures.js";
import { config } from '../../../helpers/config.js';

export function topLevelValidationTests(
    t,
    validateFn,
    validFixture,
    valueKey,
    notAllowedDataTypes,
    topFields
) {

    t.test('strict mode', t => {
        const invalidInput = {
            ...validFixture,
            extra: b4a.from('redundant field', 'utf-8')
        };
        t.absent(validateFn(invalidInput), 'Extra field should fail due to $$strict');
        t.absent(validateFn({}), 'Empty object should fail');
    });

    t.test('operation type', t => {
        const invalidOperationType = { ...validFixture, type: 'invalid-op' };
        t.absent(validateFn(invalidOperationType), 'Invalid operation type should fail');
        const notDefinedOperationType = { ...validFixture, type: 999 };
        t.absent(validateFn(notDefinedOperationType), 'Invalid operation which is not defined in OperationType should fail');

        const zeroValue = { ...validFixture, type: 0 };
        t.absent(validateFn(zeroValue), 'Type with value 0 should fail');

        const negativeValue = { ...validFixture, type: -1 };
        t.absent(validateFn(negativeValue), 'Negative type value should fail');
    });

    t.test('type range', t => {
        const belowMin = { ...validFixture, type: MIN_SAFE_VALIDATION_INTEGER - 1 };
        const aboveMax = { ...validFixture, type: MAX_SAFE_VALIDATION_INTEGER + 1 };
        t.absent(validateFn(belowMin), 'Type below allowed min should fail');
        t.absent(validateFn(aboveMax), 'Type above allowed max should fail');
    });

    t.test('neasted objects', t => {
        const nestedObjectInsideValue = {
            ...validFixture,
            [valueKey]: {
                ...validFixture[valueKey],
                nested: { foo: b4a.from('bar', 'utf-8') }
            }
        };
        t.absent(validateFn(nestedObjectInsideValue), `Unexpected nested field inside -${valueKey}- should fail due to strict`);
        const nestedObjectInsideValue2 = {
            ...validFixture,
            nested: { foo: b4a.from('bar', 'utf-8') }
        };
        //console.log('nestedObjectInsideValue2', nestedObjectInsideValue2);
        t.absent(validateFn(nestedObjectInsideValue2), 'Unexpected nested field inside general object should fail due to strict');

        const nestedObjectInsideValue3 = {
            ...validFixture,
            type: {
                foo: b4a.from('bar', 'utf-8'),
                nested: { foo: b4a.from('bar', 'utf-8') }
            }
        };
        //console.log('nestedObjectInsideValue3', nestedObjectInsideValue3);
        t.absent(validateFn(nestedObjectInsideValue3), 'Unexpected nested field inside `type` field should fail due to strict');

        const nestedObjectInsideValue4 = {
            ...validFixture,
            address: {
                foo: b4a.from('bar', 'utf-8'),
                nested: { foo: b4a.from('bar', 'utf-8') }
            }
        };
        //console.log('nestedObjectInsideValue4', nestedObjectInsideValue4);
        t.absent(validateFn(nestedObjectInsideValue4), 'Unexpected nested field inside `address` field should fail due to strict');


    });

    t.test('invalid data types', t => {
        for (const invalidDataType of notAllowedDataTypes) {
            const invalidDataTypeForType = { ...validFixture, type: invalidDataType };
            if (typeof invalidDataType === 'number') {
                continue;
            }
            //console.log('invalidDataTypeForType', invalidDataTypeForType);
            t.absent(validateFn(invalidDataTypeForType), `Invalid data type for 'type' key ${String(invalidDataType)} (${typeof invalidDataType}) should fail`);
        }

        // testing for invalid data types in address
        for (const invalidDataType of notAllowedDataTypes) {
            const invalidTypForAddressKey = { ...validFixture, address: invalidDataType };
            //console.log('invalidTypForAddressKey', invalidTypForAddressKey);
            t.absent(validateFn(invalidTypForAddressKey), `Invalid data type for 'address' key ${String(invalidDataType)} (${typeof invalidDataType}) should fail`);
        }

        for (const invalidDataType of notAllowedDataTypes) {
            if (String(invalidDataType) === '[object Object]') {
                continue;
            }
            const invalidTypeForTypeValue = { ...validFixture, [valueKey]: invalidDataType };
            //console.log('invalidTypeForTypeValue', invalidTypeForTypeValue);
            t.absent(validateFn(invalidTypeForTypeValue), `Invalid data type for ${valueKey} key ${String(invalidDataType)} (${typeof invalidDataType}) should fail`);
        }

        const invalidOperationTypeDiffType = { ...validFixture, type: "string" }
        //console.log('invalidOperationTypeDiffType', invalidOperationTypeDiffType);
        t.absent(validateFn(invalidOperationTypeDiffType), 'Wrong type for `type` should fail')

        for (const mainField of topFields) {
            const missingFieldInvalidInput = { ...validFixture }
            delete missingFieldInvalidInput[mainField]
            //console.log('missingFieldInvalidInput1111', missingFieldInvalidInput);
            t.absent(validateFn(missingFieldInvalidInput), `Missing ${mainField} should fail`);
        }
    });
}

export const valueLevelValidationTest = (t, validateFn, validFixture, valueKey, valueFields, notAllowedDataTypes) => {
    for (const field of valueFields) {
        if (partial_operation_value_type.includes(valueKey) && (field === 'is' || field === 'vn' || field === 'vs')) continue;

        const missing = {
            ...validFixture,
            [valueKey]: { ...validFixture[valueKey] }
        };
        delete missing[valueKey][field];
        //console.log(missing);
        t.absent(validateFn(missing), `Missing ${valueKey}.${field} should fail`);
    }

    t.test(`Invalid data types for each field in ${valueKey}`, t => {
        for (const field of valueFields) {
            for (const invalidType of notAllowedDataTypes) {
                const withInvalidDataType = {
                    ...validFixture,
                    [valueKey]: {
                        ...validFixture[valueKey],
                        [field]: invalidType
                    }
                };
                //console.log('withInvalidDataType', withInvalidDataType);
                t.absent(validateFn(withInvalidDataType), `Invalid data type for ${valueKey}.${field}: ${String(invalidType)} (${typeof invalidType}) should fail`);
            }
        }
    });

    t.test("Empty strings for each field in value", t => {
        for (const field of valueFields) {
            const emptyStr = {
                ...validFixture,
                [valueKey]: {
                    ...validFixture[valueKey],
                    [field]: ''
                }
            };
            //console.log('emptyStr', emptyStr);
            t.absent(validateFn(emptyStr), `Empty string for ${valueKey}.${field} should fail`);
        }
    })

    t.test("Nested objects for each field in value", t => {
        for (const field of valueFields) {
            const nestedObj = {
                ...validFixture,
                [valueKey]: {
                    ...validFixture[valueKey],
                    [field]: { foo: b4a.from('bar', 'utf-8') }
                }
            };
            //console.log('nestedObj', nestedObj);
            t.absent(validateFn(nestedObj), `Nested object for ${valueKey}.${field} should fail under strict mode`);
        }
    });

    t.test("Extra field in value", t => {
        const extraInValue = {
            ...validFixture,
            [valueKey]: {
                ...validFixture[valueKey],
                extraField: b4a.from('redundant', 'utf-8')
            }
        }
        //console.log('extraInValue', extraInValue);
        t.absent(validateFn(extraInValue), 'Extra field should fail due to $$strict')
    });

    t.test("Empty object for each field in value", t => {
        for (const field of valueFields) {
            const emptyObjForField = {
                ...validFixture,
                [valueKey]: {
                    ...validFixture[valueKey],
                    [field]: {}
                }
            }
            //console.log('emptyObjForField', emptyObjForField);
            t.absent(validateFn(emptyObjForField), `Empty object for ${valueKey}.${field} should fail`)
        }
    })

}

export function addressBufferLengthTest(
    t,
    validateFn,
    validFixture,
) {
    const emptyBuffer = b4a.alloc(0);
    const oneTooShort = b4a.alloc(config.addressLength - 1, 0x01);
    const tooShort = b4a.alloc(config.addressLength - 2, 0x01);
    const exact = b4a.alloc(config.addressLength, 0x01);
    const oneTooLong = b4a.alloc(config.addressLength + 1, 0x01);
    const tooLong = b4a.alloc(config.addressLength + 2, 0x01);

    const inputs = {
        emptyBufferInput: { ...validFixture, address: emptyBuffer },
        shortInput: { ...validFixture, address: tooShort },
        oneTooShortInput: { ...validFixture, address: oneTooShort },
        exactInput: { ...validFixture, address: exact },
        oneTooLongInput: { ...validFixture, address: oneTooLong },
        longInput: { ...validFixture, address: tooLong },
    };

    //console.log('inputs', inputs);

    t.absent(validateFn(inputs.emptyBufferInput), `'address' empty buffer (length ${emptyBuffer.length}) should fail`);

    t.absent(validateFn(inputs.shortInput), `'address' too short (length ${tooShort.length}) should fail`);

    t.absent(validateFn(inputs.oneTooShortInput), `'address' one too short (length ${oneTooShort.length}) should fail`);

    t.ok(validateFn(inputs.exactInput), `'address' exact length (length ${exact.length}) should pass`);

    t.absent(validateFn(inputs.oneTooLongInput), `'address' one too long (length ${oneTooLong.length}) should fail`);

    t.absent(validateFn(inputs.longInput), `'address' too long (length ${tooLong.length}) should fail`);
}

export function fieldsBufferLengthTest(
    t,
    validateFn,
    validFixture,
    valueKey,
    requiredFieldLengthsForValue
) {
    for (const [field, expectedLen] of Object.entries(requiredFieldLengthsForValue)) {
        const emptyBuffer = b4a.alloc(0);
        const oneTooShort = b4a.alloc(expectedLen - 1, 0x01);
        const tooShort = b4a.alloc(expectedLen - 2, 0x01);
        const exact = b4a.alloc(expectedLen, 0x01);
        const oneTooLong = b4a.alloc(expectedLen + 1, 0x01);
        const tooLong = b4a.alloc(expectedLen + 2, 0x01);

        const buildValueLevel = (val) => ({
            ...validFixture,
            [valueKey]: {
                ...validFixture[valueKey],
                [field]: val
            }
        });

        const inputs = {
            emptyBufferInput: buildValueLevel(emptyBuffer),
            shortInput: buildValueLevel(tooShort),
            oneTooShortInput: buildValueLevel(oneTooShort),
            exactInput: buildValueLevel(exact),
            oneTooLongInput: buildValueLevel(oneTooLong),
            longInput: buildValueLevel(tooLong),
        };

        //console.log('inputs', inputs);


        t.absent(validateFn(inputs.emptyBufferInput), `${valueKey}.${field} empty buffer (length ${emptyBuffer.length}) should fail`);

        t.absent(validateFn(inputs.shortInput), `${valueKey}.${field} too short (length ${tooShort.length}) should fail`);

        t.absent(validateFn(inputs.oneTooShortInput), `${valueKey}.${field} one too short (length ${oneTooShort.length}) should fail`);

        t.ok(validateFn(inputs.exactInput), `${valueKey}.${field} exact length (length ${exact.length}) should pass`);

        t.absent(validateFn(inputs.oneTooLongInput), `${valueKey}.${field} one too long (length ${oneTooLong.length}) should fail`);

        t.absent(validateFn(inputs.longInput), `${valueKey}.${field} too long (length ${tooLong.length}) should fail`);
    }
}

export function partialTypeCommonTests(t, validateFn, validFixture, valueKey) {
    t.test('missing validator fields combinations for complete payload', async t => {
        t.test('missing vn only', t => {
            const test = structuredClone(validFixture)
            delete test[valueKey].vn
            const result = validateFn(test)
            //console.log('test', test);
            t.is(result, false, 'Should fail when vn is missing')
        })

        t.test('missing vs only', t => {
            const test = structuredClone(validFixture)
            delete test[valueKey].vs
            const result = validateFn(test)
            //console.log('test', test);
            t.is(result, false, 'Should fail when vs is missing')
        })

        t.test('missing va only', t => {
            const test = structuredClone(validFixture)
            delete test[valueKey].va
            const result = validateFn(test)
            //console.log('test', test);
            t.is(result, false, 'Should fail when va is missing')
        })

        t.test('missing vn and vs', t => {
            const test = structuredClone(validFixture)
            delete test[valueKey].vn
            delete test[valueKey].vs
            const result = validateFn(test)
            //console.log('test', test);
            t.is(result, false, 'Should fail when vn and vs are missing')
        })

        t.test('missing vn and va', t => {
            const test = structuredClone(validFixture)
            delete test[valueKey].vn
            delete test[valueKey].va
            const result = validateFn(test)
            //console.log('test', test);
            t.is(result, false, 'Should fail when vn and va are missing')
        })

        t.test('missing vs and va', t => {
            const test = structuredClone(validFixture)
            delete test[valueKey].vs
            delete test[valueKey].va
            const result = validateFn(test)
            //console.log('test', test);
            t.is(result, false, 'Should fail when vs and va are missing')
        })

        t.test('missing all validator fields', t => {
            const test = structuredClone(validFixture)
            delete test[valueKey].vs
            delete test[valueKey].va
            delete test[valueKey].vn
            const result = validateFn(test)
            //console.log('test', test);
            t.is(result, true, 'Should not fail when all validator fields are missing')
        })
    })

    t.test('optional fields null cases', async t => {
        t.test('vn null only', t => {
            const test = structuredClone(validFixture)
            test[valueKey].vn = null
            const result = validateFn(test)
            //console.log('test', test);
            t.is(result, false, 'Should fail when vn is null')
        })

        t.test('vs null only', t => {
            const test = structuredClone(validFixture)
            test[valueKey].vs = null
            const result = validateFn(test)
            //console.log('test', test);
            t.is(result, false, 'Should fail when vs is null')
        })

        t.test('va null only', t => {
            const test = structuredClone(validFixture)
            test[valueKey].va = null
            const result = validateFn(test)
            //console.log('test', test);
            t.is(result, false, 'Should fail when va is null')
        })

        t.test('vn and vs null', t => {
            const test = structuredClone(validFixture)
            test[valueKey].vn = null
            test[valueKey].vs = null
            const result = validateFn(test)
            //console.log('test', test);
            t.is(result, false, 'Should fail when vn and vs are null')
        })

        t.test('vn and va null', t => {
            const test = structuredClone(validFixture)
            test[valueKey].vn = null
            test[valueKey].va = null
            const result = validateFn(test)
            //console.log('test', test);
            t.is(result, false, 'Should fail when vn and va are null')
        })

        t.test('vs and va null', t => {
            const test = structuredClone(validFixture)
            test[valueKey].vs = null
            test[valueKey].va = null
            const result = validateFn(test)
            //console.log('test', test);
            t.is(result, false, 'Should fail when vs and va are null')
        })

        t.test('all validator fields null', t => {
            const test = structuredClone(validFixture)
            test[valueKey].vn = null
            test[valueKey].vs = null
            test[valueKey].va = null
            const result = validateFn(test)
            //console.log('test', test);
            t.is(result, false, 'Should fail when all validator fields are null')
        })
    })
}