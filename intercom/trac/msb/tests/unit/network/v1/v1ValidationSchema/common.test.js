import b4a from 'b4a';

export const describeValue = (value) => {
    if (value === null) return 'null';
    if (value === undefined) return 'undefined';
    if (b4a.isBuffer(value)) return `buffer(len=${value.length})`;
    if (Number.isNaN(value)) return 'NaN';
    if (value === Infinity) return 'Infinity';
    if (Array.isArray(value)) return 'array';
    if (value instanceof Map) return 'map';
    if (value instanceof Set) return 'set';
    if (value instanceof Date) return 'date';
    if (value instanceof RegExp) return 'regexp';
    if (value instanceof Error) return `error(${value.message})`;
    if (typeof value === 'bigint') return `bigint(${value})`;
    return typeof value;
};

export const assertNoThrowAndAbsent = (t, validateFn, input, message) => {
    let result;
    t.execution(() => {
        result = validateFn(input);
    }, `${message} (no throw)`);
    t.absent(result, `${message} (should fail)`);
};

export function topLevelValidationTests(
    t,
    validateFn,
    validFixture,
    valueKey,
    notAllowedDataTypes,
    topFields,
    expectedType
) {
    t.test('strict mode', t => {
        const extraTopLevel = {
            ...validFixture,
            extra: b4a.from('redundant field', 'utf-8'),
        };
        t.absent(validateFn(extraTopLevel), 'Extra field should fail due to $$strict');
        t.absent(validateFn({}), 'Empty object should fail');
    });

    t.test('operation type', t => {
        const invalidOperationType = { ...validFixture, type: 'invalid-op' };
        t.absent(validateFn(invalidOperationType), 'Invalid operation type should fail');

        const notDefinedOperationType = { ...validFixture, type: 999 };
        t.absent(validateFn(notDefinedOperationType), 'Invalid operation type should fail');

        const zeroValue = { ...validFixture, type: 0 };
        t.absent(validateFn(zeroValue), 'Type with value 0 should fail');

        const negativeValue = { ...validFixture, type: -1 };
        t.absent(validateFn(negativeValue), 'Negative type value should fail');

        const wrongKnownType = { ...validFixture, type: expectedType + 1 };
        t.absent(validateFn(wrongKnownType), 'Wrong known type should fail');
    });

    t.test('nested objects', t => {
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
        t.absent(validateFn(nestedObjectInsideValue2), 'Unexpected nested field inside general object should fail due to strict');

        const nestedObjectInsideType = {
            ...validFixture,
            type: {
                foo: b4a.from('bar', 'utf-8'),
                nested: { foo: b4a.from('bar', 'utf-8') }
            }
        };
        t.absent(validateFn(nestedObjectInsideType), 'Unexpected nested field inside `type` field should fail');

        const nestedObjectInsideId = {
            ...validFixture,
            id: {
                foo: b4a.from('bar', 'utf-8'),
                nested: { foo: b4a.from('bar', 'utf-8') }
            }
        };
        t.absent(validateFn(nestedObjectInsideId), 'Unexpected nested field inside `id` field should fail');

        const nestedObjectInsideTimestamp = {
            ...validFixture,
            timestamp: {
                foo: b4a.from('bar', 'utf-8'),
                nested: { foo: b4a.from('bar', 'utf-8') }
            }
        };
        t.absent(validateFn(nestedObjectInsideTimestamp), 'Unexpected nested field inside `timestamp` field should fail');

        const nestedObjectInsideCapabilities = {
            ...validFixture,
            capabilities: {
                foo: b4a.from('bar', 'utf-8'),
                nested: { foo: b4a.from('bar', 'utf-8') }
            }
        };
        t.absent(validateFn(nestedObjectInsideCapabilities), 'Unexpected nested field inside `capabilities` field should fail');
    });

    t.test('invalid data types', t => {
        for (const invalidDataType of notAllowedDataTypes) {
            if (typeof invalidDataType !== 'number') {
                const invalidTypeForType = { ...validFixture, type: invalidDataType };
                t.absent(
                    validateFn(invalidTypeForType),
                    `Invalid data type for 'type' key ${describeValue(invalidDataType)} should fail`
                );
            }

            if (typeof invalidDataType !== 'string') {
                const invalidTypeForId = { ...validFixture, id: invalidDataType };
                t.absent(
                    validateFn(invalidTypeForId),
                    `Invalid data type for 'id' key ${describeValue(invalidDataType)} should fail`
                );
            }

            if (typeof invalidDataType !== 'number') {
                const invalidTypeForTimestamp = { ...validFixture, timestamp: invalidDataType };
                t.absent(
                    validateFn(invalidTypeForTimestamp),
                    `Invalid data type for 'timestamp' key ${describeValue(invalidDataType)} should fail`
                );
            }

            if (!Array.isArray(invalidDataType)) {
                const invalidTypeForCapabilities = { ...validFixture, capabilities: invalidDataType };
                t.absent(
                    validateFn(invalidTypeForCapabilities),
                    `Invalid data type for 'capabilities' key ${describeValue(invalidDataType)} should fail`
                );
            }

            if (String(invalidDataType) !== '[object Object]') {
                const invalidTypeForPayload = { ...validFixture, [valueKey]: invalidDataType };
                t.absent(
                    validateFn(invalidTypeForPayload),
                    `Invalid data type for ${valueKey} key ${describeValue(invalidDataType)} should fail`
                );
            }
        }

        const invalidOperationTypeDiffType = { ...validFixture, type: "string" }
        t.absent(validateFn(invalidOperationTypeDiffType), 'Wrong type for `type` should fail')

        for (const mainField of topFields) {
            const missingFieldInvalidInput = { ...validFixture }
            delete missingFieldInvalidInput[mainField]
            t.absent(validateFn(missingFieldInvalidInput), `Missing ${mainField} should fail`);
        }
    });
}

export function headerFieldValueValidationTests(t, validateFn, validFixture) {
    t.absent(
        validateFn({...validFixture, id: ''}),
        'empty id should fail'
    );

    t.absent(
        validateFn({...validFixture, id: 'x'.repeat(65)}),
        'id longer than 64 chars should fail'
    );

    t.absent(
        validateFn({...validFixture, timestamp: 0}),
        'timestamp 0 should fail'
    );

    t.absent(
        validateFn({...validFixture, timestamp: -1}),
        'negative timestamp should fail'
    );

    t.absent(
        validateFn({...validFixture, timestamp: 1.1}),
        'non-integer timestamp should fail'
    );

    t.absent(
        validateFn({...validFixture, timestamp: NaN}),
        'timestamp NaN should fail'
    );

    t.absent(
        validateFn({...validFixture, timestamp: Infinity}),
        'timestamp Infinity should fail'
    );

    t.absent(
        validateFn({...validFixture, timestamp: Number.MAX_SAFE_INTEGER + 1}),
        'timestamp above MAX_SAFE_INTEGER should fail'
    );

    t.absent(
        validateFn({...validFixture, capabilities: 'cap:a'}),
        'capabilities must be an array'
    );

    t.absent(
        validateFn({...validFixture, capabilities: [1]}),
        'capabilities items must be strings'
    );
}

export function valueLevelValidationTests(
    t,
    validateFn,
    validFixture,
    valueKey,
    valueFields,
    notAllowedDataTypes,
    opts = {}
) {
    const skipInvalidType = opts.skipInvalidType || (() => false);

    for (const field of valueFields) {
        const missing = structuredClone(validFixture);
        delete missing[valueKey][field];
        t.absent(validateFn(missing), `Missing ${valueKey}.${field} should fail`);
    }

    t.test(`Invalid data types for each field in ${valueKey}`, t => {
        for (const field of valueFields) {
            for (const invalidType of notAllowedDataTypes) {
                if (skipInvalidType(field, invalidType)) continue;

                const withInvalidDataType = structuredClone(validFixture);
                withInvalidDataType[valueKey][field] = invalidType;
                t.absent(
                    validateFn(withInvalidDataType),
                    `Invalid data type for ${valueKey}.${field}: ${describeValue(invalidType)} should fail`
                );
            }
        }
    });

    t.test("Extra field in value", t => {
        const extraInValue = structuredClone(validFixture);
        extraInValue[valueKey].extraField = b4a.from('redundant', 'utf-8');
        t.absent(validateFn(extraInValue), 'Extra field should fail due to strict')
    });
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

        t.absent(validateFn(inputs.emptyBufferInput), `${valueKey}.${field} empty buffer (length ${emptyBuffer.length}) should fail`);
        t.absent(validateFn(inputs.shortInput), `${valueKey}.${field} too short (length ${tooShort.length}) should fail`);
        t.absent(validateFn(inputs.oneTooShortInput), `${valueKey}.${field} one too short (length ${oneTooShort.length}) should fail`);
        t.ok(validateFn(inputs.exactInput), `${valueKey}.${field} exact length (length ${exact.length}) should pass`);
        t.absent(validateFn(inputs.oneTooLongInput), `${valueKey}.${field} one too long (length ${oneTooLong.length}) should fail`);
        t.absent(validateFn(inputs.longInput), `${valueKey}.${field} too long (length ${tooLong.length}) should fail`);
    }
}

export function fieldsNonZeroBufferTest(t, validateFn, validFixture, valueKey, fields) {
    for (const field of fields) {
        const zeroFilled = structuredClone(validFixture);
        const len = zeroFilled[valueKey][field]?.length ?? 0;
        zeroFilled[valueKey][field] = b4a.alloc(len, 0);
        t.absent(validateFn(zeroFilled), `${valueKey}.${field} all-zero buffer should fail`);
    }
}
