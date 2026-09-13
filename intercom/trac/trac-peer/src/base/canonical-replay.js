import b4a from 'b4a';
import { jsonStringify } from '../utils/types.js';

// Capabilities never come from a dispatch or RPC field. They are minted only
// after the operation handler has authenticated a consensus input and the
// signed canonical view proves that exact operation was previously accepted.
const evidence = new WeakMap();
const same = (a, b) => jsonStringify(a) === jsonStringify(b);
export async function canonicalReplayContext(op, storage, node, canonicalView) {
    if ((op?.type !== 'feature' && op?.type !== 'tx') || node?.value !== op ||
        node.optimistic === true || !Number.isSafeInteger(node.length) || node.length < 1 ||
        !node.from || !Number.isSafeInteger(node.from.signedLength) ||
        node.from.signedLength < node.length || typeof node.from.get !== 'function' ||
        !Number.isSafeInteger(canonicalView?.core?.signedLength) ||
        canonicalView.core.signedLength < 1 || typeof canonicalView.checkout !== 'function') return null;
    // Bind the full envelope (including version, kind, address and nonce) to
    // authenticated input bytes, not only to a signature that covers its value.
    const original = await node.from.get(node.length - 1, { wait: false });
    if (!b4a.isBuffer(original?.node?.value)) return null;
    let decoded;
    try { decoded = JSON.parse(b4a.toString(original.node.value)); } catch { return null; }
    if (!same(decoded, op)) return null;
    const signedLength = canonicalView.core.signedLength;
    const view = canonicalView.checkout(signedLength);
    try {
        if (op.type === 'feature') {
            const dispatch = op.value?.dispatch;
            if (!dispatch?.hash || dispatch.type !== 'mayhem_feature') return null;
            const seen = await view.get(`sh/${dispatch.hash}`);
            const result = (await view.get(`fr/${dispatch.hash}`))?.value;
            if (!seen || !result || result.type !== 'feature_result' ||
                result.hash !== dispatch.hash || result.feature_key !== dispatch.key ||
                result.address !== dispatch.address) return null;
        } else {
            const index = (await view.get(`tx/${op.key}`))?.value;
            if (!Number.isSafeInteger(index) || index < 0) return null;
            const record = (await view.get(`txi/${index}`))?.value;
            if (!record || record.tx !== op.key || record.ipk !== op.value.ipk ||
                record.wp !== op.value.wp || !same(record.val, op.value.dispatch)) return null;
        }
        const token = Object.freeze({});
        evidence.set(token, { op, storage, encoded: jsonStringify(op), signedLength });
        return token;
    } finally { await view.close(); }
}

export function consumeCanonicalReplayContext(token, op, storage) {
    const bound = token && evidence.get(token);
    if (!bound) return false;
    evidence.delete(token);
    return bound.op === op && bound.storage === storage && bound.encoded === jsonStringify(op)
        ? { signedLength: bound.signedLength } : false;
}
