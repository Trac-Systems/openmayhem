import test from 'brittle'
import Check from '../../../../src/utils/check.js';
import { TXO, not_allowed_data_types } from '../../../fixtures/check.fixtures.js';
import { topLevelValidationTests, valueLevelValidationTest, addressBufferLengthTest, fieldsBufferLengthTest, partialTypeCommonTests } from './common.test.js';
import { config } from '../../../helpers/config.js';

const check = new Check(config)

test('validateTransactionOperation - happy-path case', t => {
    // ADD_WRITER
    const partial_result = check.validateTransactionOperation(TXO.valid_partial_transaction_operation)
    const complete_result = check.validateTransactionOperation(TXO.valid_complete_transaction_operation)
    t.ok(partial_result, 'Valid data for partial transaction operation should pass the validation')
    t.ok(complete_result, 'Valid data for complete transaction operation should pass the validation')
})

test('validateTransactionOperation - optional fields va, vn, vs', t => {
    partialTypeCommonTests(
        t,
        check.validateTransactionOperation.bind(check),
        TXO.valid_complete_transaction_operation,
        'txo'

    )
})

test('validateTransactionOperation - data type validation TOP LEVEL', t => {
    //complete
    topLevelValidationTests(
        t,
        check.validateTransactionOperation.bind(check),
        TXO.valid_complete_transaction_operation,
        'txo',
        not_allowed_data_types,
        TXO.top_fields_transaction_operation
    );
    //partial
    topLevelValidationTests(
        t,
        check.validateTransactionOperation.bind(check),
        TXO.valid_partial_transaction_operation,
        'txo',
        not_allowed_data_types,
        TXO.top_fields_transaction_operation
    );
});


test('validateTransactionOperation - value level validation (txo)', t => {
    //complete
    valueLevelValidationTest(
        t,
        check.validateTransactionOperation.bind(check),
        TXO.valid_complete_transaction_operation,
        'txo',
        TXO.complete_transaction_operation_value_fields,
        not_allowed_data_types
    );
    // partial
    valueLevelValidationTest(
        t,
        check.validateTransactionOperation.bind(check),
        TXO.valid_partial_transaction_operation,
        'txo',
        TXO.partial_transaction_operation_value_fields,
        not_allowed_data_types
    );

});



test('validateTransactionOperation - address buffer length validation - TOP LEVEL', t => {
    //complete
    addressBufferLengthTest(
        t,
        check.validateTransactionOperation.bind(check),
        TXO.valid_complete_transaction_operation,
    );
    //partial
    addressBufferLengthTest(
        t,
        check.validateTransactionOperation.bind(check),
        TXO.valid_partial_transaction_operation,
    );
});


test('validateTransactionOperation - Buffer length validation - VALUE LEVEL (txo)', t => {
    //complete
    fieldsBufferLengthTest(
        t,
        check.validateTransactionOperation.bind(check),
        TXO.valid_complete_transaction_operation,
        'txo',
        TXO.required_length_of_fields_for_complete_transaction_operation
    );
    //partial
    fieldsBufferLengthTest(
        t,
        check.validateTransactionOperation.bind(check),
        TXO.valid_partial_transaction_operation,
        'txo',
        TXO.required_length_of_fields_for_partial_transaction_operation
    );

});
