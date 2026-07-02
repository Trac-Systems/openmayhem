import test from 'brittle'
import Check from '../../../../src/utils/check.js'
import { CAO, not_allowed_data_types } from '../../../fixtures/check.fixtures.js'
import { topLevelValidationTests, valueLevelValidationTest, addressBufferLengthTest, fieldsBufferLengthTest } from './common.test.js';
import { config } from '../../../helpers/config.js';

const check = new Check(config)

test('validateCoreAdminOperation- happy paths for all operation types', t => {
    const validInputs = [
        CAO.validAddAdminOperation,
        CAO.validDisableInitializationOperation,
    ]

    for (const validInput of validInputs) {
        t.ok(check.validateCoreAdminOperation(validInput), `Valid payload for ${validInput.type} should pass`)
    }
})

test('validateCoreAdminOperation - data type validation TOP LEVEL', t => {
    // ADD_ADMIN
    topLevelValidationTests(
        t,
        check.validateCoreAdminOperation.bind(check),
        CAO.validAddAdminOperation,
        'cao',
        not_allowed_data_types,
        CAO.topFieldsCoreAdmin
    );
    // DISABLE_INITIALIZATION
    topLevelValidationTests(
        t,
        check.validateCoreAdminOperation.bind(check),
        CAO.validDisableInitializationOperation,
        'cao',
        not_allowed_data_types,
        CAO.topFieldsCoreAdmin
    );
})

test('validateCoreAdminOperation - value level validation (cao)', t => {
    // ADD_ADMIN
    valueLevelValidationTest(
        t,
        check.validateCoreAdminOperation.bind(check),
        CAO.validAddAdminOperation,
        'cao',
        CAO.coreAdminOperationValuesFields,
        not_allowed_data_types
    );
    // DISABLE_INITIALIZATION
    valueLevelValidationTest(
        t,
        check.validateCoreAdminOperation.bind(check),
        CAO.validDisableInitializationOperation,
        'cao',
        CAO.coreAdminOperationValuesFields,
        not_allowed_data_types
    );

})

test('validateCoreAdminOperation - address buffer length validation - TOP LEVEL', t => {
    // ADD_ADMIN
    addressBufferLengthTest(
        t,
        check.validateCoreAdminOperation.bind(check),
        CAO.validAddAdminOperation,
    );
    // DISABLE_INITIALIZATION
    addressBufferLengthTest(
        t,
        check.validateCoreAdminOperation.bind(check),
        CAO.validDisableInitializationOperation,
    );
});

test('validateCoreAdminOperation - fields buffer length validation - VALUE LEVEL (eko)', t => {
    // ADD_ADMIN
    fieldsBufferLengthTest(
        t,
        check.validateCoreAdminOperation.bind(check),
        CAO.validAddAdminOperation,
        'cao',
        CAO.requiredLengthOfFieldsForCoreAdmin
    )
    // DISABLE_INITIALIZATION
    fieldsBufferLengthTest(
        t,
        check.validateCoreAdminOperation.bind(check),
        CAO.validDisableInitializationOperation,
        'cao',
        CAO.requiredLengthOfFieldsForCoreAdmin
    )
});