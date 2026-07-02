import test from 'brittle'

import Check from '../../../../src/utils/check.js';
import { RAO, not_allowed_data_types } from '../../../fixtures/check.fixtures.js';
import { topLevelValidationTests, valueLevelValidationTest, addressBufferLengthTest, fieldsBufferLengthTest, partialTypeCommonTests } from './common.test.js';
import { config } from '../../../helpers/config.js';

const check = new Check(config)

test('validateRoleAccessOperation - happy-path case', t => {
    // ADD_WRITER
    t.ok(check.validateRoleAccessOperation(RAO.valid_partial_add_writer), 'Valid data for partial ADD_WRITER operation should pass the validation')
    t.ok(check.validateRoleAccessOperation(RAO.valid_complete_add_writer), 'Valid data for complete ADD_WRITER operation should pass the validation')

    // REMOVE_WRITER
    t.ok(check.validateRoleAccessOperation(RAO.valid_partial_remove_writer), 'Valid data for partial REMOVE_WRITER operation should pass the validation')
    t.ok(check.validateRoleAccessOperation(RAO.valid_complete_remove_writer), 'Valid data for complete REMOVE_WRITER operation should pass the validation')

    // ADMIN_RECOVERY
    t.ok(check.validateRoleAccessOperation(RAO.valid_partial_admin_recovery), 'Valid data for partial ADMIN_RECOVERY operation should pass the validation')
    t.ok(check.validateRoleAccessOperation(RAO.valid_complete_admin_recovery), 'Valid data for complete ADMIN_RECOVERY operation should pass the validation')
})

test('validateRoleAccessOperation - type level validation (rao)', t => {
    // ADD_WRITER complete
    topLevelValidationTests(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_complete_add_writer,
        'rao',
        not_allowed_data_types,
        RAO.top_fields_role_access
    )

    // ADD_WRITER partial
    topLevelValidationTests(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_partial_add_writer,
        'rao',
        not_allowed_data_types,
        RAO.top_fields_role_access
    )

    // REMOVE_WRITER complete
    topLevelValidationTests(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_complete_remove_writer,
        'rao',
        not_allowed_data_types,
        RAO.top_fields_role_access
    )

    // REMOVE_WRITER partial
    topLevelValidationTests(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_partial_remove_writer,
        'rao',
        not_allowed_data_types,
        RAO.top_fields_role_access
    )

    // ADMIN_RECOVERY complete
    topLevelValidationTests(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_complete_admin_recovery,
        'rao',
        not_allowed_data_types,
        RAO.top_fields_role_access
    )

    // ADMIN_RECOVERY partial
    topLevelValidationTests(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_partial_admin_recovery,
        'rao',
        not_allowed_data_types,
        RAO.top_fields_role_access
    )
})

test('validateRoleAccessOperation - value level validation (rao)', t => {
    // ADD_WRITER complete
    valueLevelValidationTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_complete_add_writer,
        'rao',
        RAO.complete_role_access_value_fields,
        not_allowed_data_types
    )

    // ADD_WRITER partial
    valueLevelValidationTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_partial_add_writer,
        'rao',
        RAO.partial_role_access_value_fields,
        not_allowed_data_types
    )

    // REMOVE_WRITER complete
    valueLevelValidationTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_complete_remove_writer,
        'rao',
        RAO.complete_role_access_value_fields,
        not_allowed_data_types
    )

    // REMOVE_WRITER partial
    valueLevelValidationTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_partial_remove_writer,
        'rao',
        RAO.partial_role_access_value_fields,
        not_allowed_data_types
    )

    // ADMIN_RECOVERY complete
    valueLevelValidationTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_complete_admin_recovery,
        'rao',
        RAO.complete_role_access_value_fields,
        not_allowed_data_types
    )

    // ADMIN_RECOVERY partial
    valueLevelValidationTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_partial_admin_recovery,
        'rao',
        RAO.partial_role_access_value_fields,
        not_allowed_data_types
    )
})

test('validateRoleAccessOperation - address buffer length validation - TOP LEVEL', t => {
    // ADD_WRITER complete
    addressBufferLengthTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_complete_add_writer
    )

    // ADD_WRITER partial
    addressBufferLengthTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_partial_add_writer
    )

    // REMOVE_WRITER complete
    addressBufferLengthTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_complete_remove_writer
    )

    // REMOVE_WRITER partial
    addressBufferLengthTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_partial_remove_writer
    )

    // ADMIN_RECOVERY complete
    addressBufferLengthTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_complete_admin_recovery
    )

    // ADMIN_RECOVERY partial
    addressBufferLengthTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_partial_admin_recovery
    )
})

test('validateRoleAccessOperation - fields buffer length validation - VALUE LEVEL (rao)', t => {
    // ADD_WRITER complete
    fieldsBufferLengthTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_complete_add_writer,
        'rao',
        RAO.required_length_of_fields_for_complete_role_access
    )

    // ADD_WRITER partial
    fieldsBufferLengthTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_partial_add_writer,
        'rao',
        RAO.required_length_of_fields_for_partial_role_access
    )

    // REMOVE_WRITER complete
    fieldsBufferLengthTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_complete_remove_writer,
        'rao',
        RAO.required_length_of_fields_for_complete_role_access
    )

    // REMOVE_WRITER partial
    fieldsBufferLengthTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_partial_remove_writer,
        'rao',
        RAO.required_length_of_fields_for_partial_role_access
    )

    // ADMIN_RECOVERY complete
    fieldsBufferLengthTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_complete_admin_recovery,
        'rao',
        RAO.required_length_of_fields_for_complete_role_access
    )

    // ADMIN_RECOVERY partial
    fieldsBufferLengthTest(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_partial_admin_recovery,
        'rao',
        RAO.required_length_of_fields_for_partial_role_access
    )
})

test('validateRoleAccessOperation - optional fields va, vn, vs', t => {
    partialTypeCommonTests(
        t,
        check.validateRoleAccessOperation.bind(check),
        RAO.valid_complete_add_writer,
        'rao'
    )
})
