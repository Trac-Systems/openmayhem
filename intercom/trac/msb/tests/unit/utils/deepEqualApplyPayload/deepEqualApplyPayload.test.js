import test from 'brittle';
import b4a from 'b4a';

import fixtures from '../../../fixtures/protobuf.fixtures.js';
import { isDeepEqualApplyPayload } from '../../../../src/utils/deepEqualApplyPayload.js';

const applyPayloads = new Map([
    ['txComplete', fixtures.validTransactionOperation],
    ['txPartial', fixtures.validPartialTransactionOperation],
    ['addIndexer', fixtures.validAddIndexer],
    ['removeIndexer', fixtures.validRemoveIndexer],
    ['appendWhitelist', fixtures.validAppendWhitelist],
    ['banValidator', fixtures.validBanValidator],
    ['addAdmin', fixtures.validAddAdmin],
    ['addWriterComplete', fixtures.validCompleteAddWriter],
    ['addWriterPartial', fixtures.validPartialAddWriter],
    ['removeWriterComplete', fixtures.validCompleteRemoveWriter],
    ['removeWriterPartial', fixtures.validPartialRemoveWriter],
    ['adminRecoveryComplete', fixtures.validCompleteAdminRecovery],
    ['adminRecoveryPartial', fixtures.validPartialAdminRecovery],
    ['bootstrapDeploymentComplete', fixtures.validCompleteBootstrapDeployment],
    ['bootstrapDeploymentPartial', fixtures.validPartialBootstrapDeployment],
    ['transferComplete', fixtures.validTransferOperation],
    ['transferPartial', fixtures.validPartialTransferOperation],
    ['balanceInitialization', fixtures.validBalanceInitOperation],
    ['disableInitialization', fixtures.validDisableInitialization],
]);

const cloneDeep = (value) => {
    if (b4a.isBuffer(value)) return b4a.from(value);

    if (Array.isArray(value)) {
        return value.map(cloneDeep);
    }

    if (value && typeof value === 'object') {
        const clone = {};
        for (const key of Object.keys(value)) {
            clone[key] = cloneDeep(value[key]);
        }
        return clone;
    }

    return value;
};

const mutateFirstBuffer = (value) => {
    if (!value || typeof value !== 'object') return false;

    if (Array.isArray(value)) {
        for (let i = 0; i < value.length; i++) {
            if (mutateFirstBuffer(value[i])) return true;
        }
        return false;
    }

    for (const key of Object.keys(value)) {
        const current = value[key];

        if (b4a.isBuffer(current)) {
            if (current.length === 0) {
                value[key] = b4a.from([0x01]);
                return true;
            }

            current[0] = current[0] ^ 0xff;
            return true;
        }

        if (current && typeof current === 'object') {
            if (mutateFirstBuffer(current)) return true;
        }
    }

    return false;
};

test('isDeepEqualApplyPayload returns true for deep-equal apply payload fixtures', t => {
    for (const [name, payload] of applyPayloads) {
        const cloned = cloneDeep(payload);
        t.ok(isDeepEqualApplyPayload(payload, cloned), `${name} should be deep-equal`);
    }
});

test('isDeepEqualApplyPayload returns false when payload content differs', t => {
    for (const [name, payload] of applyPayloads) {
        const mutated = cloneDeep(payload);
        const didMutate = mutateFirstBuffer(mutated);

        t.ok(didMutate, `${name} mutation should modify at least one buffer`);
        t.absent(isDeepEqualApplyPayload(payload, mutated), `${name} should not be deep-equal after mutation`);
    }
});

test('isDeepEqualApplyPayload handles primitive and structural mismatches', t => {
    t.ok(isDeepEqualApplyPayload(null, null), 'null equals null');
    t.absent(isDeepEqualApplyPayload(null, undefined), 'null and undefined differ');
    t.absent(isDeepEqualApplyPayload(1, '1'), 'number and string differ');
    t.absent(isDeepEqualApplyPayload([], {}), 'array and object differ');
    t.absent(isDeepEqualApplyPayload({ type: 1 }, { type: 1, extra: true }), 'missing/extra keys differ');
});

