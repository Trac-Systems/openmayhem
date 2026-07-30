import {
    setupBootstrapDeploymentScenario,
    buildBootstrapDeploymentPayload,
    assertBootstrapDeploymentFailureState
} from './bootstrapDeploymentScenarioHelpers.js';
import { safeDecodeApplyOperation, safeEncodeApplyOperation } from '../../../../../src/utils/protobuf/operationHelpers.js';
import OperationValidationScenarioBase from '../common/base/OperationValidationScenarioBase.js';

function removeValidatorFields(t, validPayload) {
    const decoded = safeDecodeApplyOperation(validPayload);
    t.ok(decoded, 'valid payload decodes before mutation');
    if (!decoded?.bdo) return validPayload;
    const mutated = safeEncodeApplyOperation({
        type: decoded.type,
        address: decoded.address,
        bdo: {
            tx: decoded.bdo.tx,
            txv: decoded.bdo.txv,
            bs: decoded.bdo.bs,
            ic: decoded.bdo.ic,
            in: decoded.bdo.in,
            is: decoded.bdo.is
        }
    });
    const roundtrip = safeDecodeApplyOperation(mutated);
    t.ok(roundtrip, 'mutated payload decodes');
    return mutated;
}

export default function bootstrapDeploymentIncompleteOperationScenario() {
    new OperationValidationScenarioBase({
        title: 'State.apply bootstrapDeployment rejects operations missing validator cosignature',
        setupScenario: setupBootstrapDeploymentScenario,
        buildValidPayload: buildBootstrapDeploymentPayload,
        mutatePayload: removeValidatorFields,
        applyInvalidPayload: async (context, invalidPayload) => {
            const { bootstrap } = context;
            await bootstrap.base.append(invalidPayload);
            await bootstrap.base.update();
        },
        assertStateUnchanged: (t, context, _validPayload, invalidPayload) =>
            assertBootstrapDeploymentFailureState(t, context, { payload: invalidPayload }),
        expectedLogs: ['Contract schema validation failed.']
    }).performScenario();
}
