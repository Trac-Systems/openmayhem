import test from 'brittle'
import Check from '../../../../src/utils/check.js'
import { ACO, not_allowed_data_types } from '../../../fixtures/check.fixtures.js'
import { topLevelValidationTests, valueLevelValidationTest, addressBufferLengthTest, fieldsBufferLengthTest } from './common.test.js';
import { config } from '../../../helpers/config.js';

const check = new Check(config)

test('validateAdminControlOperation- happy paths for all operation types', t => {
    const validInputs = [
        ACO.validAppendWhitelistOperation,
        ACO.validAddIndexerOperation,
        ACO.validRemoveIndexerOperation,
        ACO.validBanValidatorOperation,
    ]

    for (const validInput of validInputs) {
        t.ok(check.validateAdminControlOperation(validInput), `Valid payload for ${validInput.type} should pass`)
    }
})

test('validateAdminControlOperation - type level validation (aco)', t => {
    topLevelValidationTests(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validAppendWhitelistOperation,
        'aco',
        not_allowed_data_types,
        ACO.topFieldsAdminControl
    );

    topLevelValidationTests(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validAddIndexerOperation,
        'aco',
        not_allowed_data_types,
        ACO.topFieldsAdminControl
    );

    topLevelValidationTests(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validRemoveIndexerOperation,
        'aco',
        not_allowed_data_types,
        ACO.topFieldsAdminControl
    );

    topLevelValidationTests(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validBanValidatorOperation,
        'aco',
        not_allowed_data_types,
        ACO.topFieldsAdminControl
    );
});

test('validateAdminControlOperation - value level validation (aco)', t => {
    valueLevelValidationTest(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validAppendWhitelistOperation,
        'aco',
        ACO.adminControlValueFields,
        not_allowed_data_types
    );

    valueLevelValidationTest(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validAddIndexerOperation,
        'aco',
        ACO.adminControlValueFields,
        not_allowed_data_types
    );

    valueLevelValidationTest(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validRemoveIndexerOperation,
        'aco',
        ACO.adminControlValueFields,
        not_allowed_data_types
    );

    valueLevelValidationTest(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validBanValidatorOperation,
        'aco',
        ACO.adminControlValueFields,
        not_allowed_data_types
    );
});

test('validateAdminControlOperation - address buffer length validation - TOP LEVEL', t => {
    addressBufferLengthTest(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validAppendWhitelistOperation,
    );

    addressBufferLengthTest(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validAddIndexerOperation,
    );

    addressBufferLengthTest(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validRemoveIndexerOperation,
    );

    addressBufferLengthTest(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validBanValidatorOperation,
    );
});

test('validateAdminControlOperation - fields buffer length validation - VALUE LEVEL (aco)', t => {
    fieldsBufferLengthTest(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validAppendWhitelistOperation,
        'aco',
        ACO.requiredLengthOfFieldsForAdminControl
    );

    fieldsBufferLengthTest(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validAddIndexerOperation,
        'aco',
        ACO.requiredLengthOfFieldsForAdminControl
    );

    fieldsBufferLengthTest(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validRemoveIndexerOperation,
        'aco',
        ACO.requiredLengthOfFieldsForAdminControl
    );

    fieldsBufferLengthTest(
        t,
        check.validateAdminControlOperation.bind(check),
        ACO.validBanValidatorOperation,
        'aco',
        ACO.requiredLengthOfFieldsForAdminControl
    );
});