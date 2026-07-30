import { test } from 'brittle';

import {
    resultToValidatorAction,
    shouldEndConnection,
    SENDER_ACTION
} from '../../../../src/core/network/protocols/connectionPolicies.js';
import { ResultCode } from '../../../../src/utils/constants.js';

test('connectionPolicies maps OK to SUCCESS', t => {
    t.is(
        resultToValidatorAction(ResultCode.OK),
        SENDER_ACTION.SUCCESS
    );
});

test('connectionPolicies maps TX_ALREADY_PENDING to NO_ROTATE', t => {
    t.is(
        resultToValidatorAction(ResultCode.TX_ALREADY_PENDING),
        SENDER_ACTION.NO_ROTATE
    );
});

test('connectionPolicies maps TX_COMMITTED_RECEIPT_MISSING to ROTATE', t => {
    t.is(
        resultToValidatorAction(ResultCode.TX_COMMITTED_RECEIPT_MISSING),
        SENDER_ACTION.ROTATE
    );
});

test('connectionPolicies maps TIMEOUT to ROTATE for sender-side retry logic', t => {
    t.is(
        resultToValidatorAction(ResultCode.TIMEOUT),
        SENDER_ACTION.ROTATE
    );
});

test('connectionPolicies keeps validator connection open for TIMEOUT responses', t => {
    t.is(
        shouldEndConnection(ResultCode.TIMEOUT),
        false
    );
});

test('connectionPolicies maps unknown result code to UNDEFINED', t => {
    t.is(
        resultToValidatorAction(999999),
        SENDER_ACTION.UNDEFINED
    );
});

test('connectionPolicies maps every ResultCode constant to a concrete sender action', t => {
    const unassigned = [];

    for (const [name, code] of Object.entries(ResultCode)) {
        const action = resultToValidatorAction(code);
        if (action === SENDER_ACTION.UNDEFINED) {
            unassigned.push(`${name}(${code})`);
        }
    }

    t.alike(
        unassigned,
        [],
        `Unassigned ResultCode entries in policy: ${unassigned.join(', ')}`
    );
});
