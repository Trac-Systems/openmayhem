import test from 'brittle';
import b4a from 'b4a';

import V1LivenessResponse from '../../../../src/core/network/protocols/v1/validators/V1LivenessResponse.js';
import { config } from '../../../helpers/config.js';
import { errorMessageIncludes } from '../../../helpers/regexHelper.js';

test('V1LivenessResponse.validate runs all validation steps in order', async t => {
    const validator = new V1LivenessResponse(config);
    const calls = [];

    validator.isPayloadSchemaValid = () => calls.push('schema');
    validator.validateResponseType = () => calls.push('responseType');
    validator.validatePeerCorrectness = () => calls.push('peer');
    validator.validateSignature = async () => calls.push('signature');

    const result = await validator.validate(
        {},
        { remotePublicKey: b4a.alloc(32, 1) },
        { requestedTo: b4a.toString(b4a.alloc(32, 1), 'hex') }
    );

    t.is(result, true);
    t.alike(calls, ['schema', 'responseType', 'peer', 'signature']);
});

test('V1LivenessResponse.validate propagates response-type errors', async t => {
    const validator = new V1LivenessResponse(config);

    validator.isPayloadSchemaValid = () => true;
    validator.validateResponseType = () => {
        throw new Error('invalid response type');
    };
    validator.validatePeerCorrectness = () => true;
    validator.validateSignature = async () => true;

    await t.exception(
        async () => validator.validate(
            {},
            { remotePublicKey: b4a.alloc(32, 1) },
            { requestedTo: b4a.toString(b4a.alloc(32, 1), 'hex') }
        ),
        errorMessageIncludes('invalid response type')
    );
});
