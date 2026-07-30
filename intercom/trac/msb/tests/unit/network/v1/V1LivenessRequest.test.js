import test from 'brittle';
import b4a from 'b4a';

import V1LivenessRequest from '../../../../src/core/network/protocols/v1/validators/V1LivenessRequest.js';
import { config } from '../../../helpers/config.js';
import { errorMessageIncludes } from '../../../helpers/regexHelper.js';

test('V1LivenessRequest.validate runs schema and signature validation', async t => {
    const validator = new V1LivenessRequest(config);
    const calls = [];

    validator.isPayloadSchemaValid = () => calls.push('schema');
    validator.validateSignature = async () => calls.push('signature');

    const result = await validator.validate({}, b4a.alloc(32, 1));

    t.is(result, true);
    t.alike(calls, ['schema', 'signature']);
});

test('V1LivenessRequest.validate propagates signature validation errors', async t => {
    const validator = new V1LivenessRequest(config);
    validator.isPayloadSchemaValid = () => true;
    validator.validateSignature = async () => {
        throw new Error('signature failed');
    };

    await t.exception(
        async () => validator.validate({}, b4a.alloc(32, 1)),
        errorMessageIncludes('signature failed')
    );
});
