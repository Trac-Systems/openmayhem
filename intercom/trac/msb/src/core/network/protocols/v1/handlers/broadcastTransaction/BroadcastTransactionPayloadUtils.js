import b4a from "b4a";

import { operationToPayload } from "../../../../../../utils/applyOperations.js";

const COMPLETION_FIELDS = ['va', 'vn', 'vs'];

export function sanitizeDecodedPartialTransaction(decodedTransaction) {
    const operationKey = operationToPayload(decodedTransaction?.type);
    const operation = decodedTransaction?.[operationKey];

    if (!operation || typeof operation !== 'object') {
        return;
    }

    for (const completionField of COMPLETION_FIELDS) {
        // Protobuf decode sets optional completion fields as null, but partial validators expect those fields to be absent.
        // Otherwise, the presence of null fields causes validation to fail with "Expected type X but got null" errors.
        if (operation[completionField] === null) {
            delete operation[completionField];
        }
    }
}

export function getTxHashFromDecodedTransaction(decodedTransaction, payloadKey = operationToPayload(decodedTransaction?.type)) {
    return b4a.toString(decodedTransaction[payloadKey].tx, 'hex');
}
