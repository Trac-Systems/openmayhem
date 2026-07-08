import { BaseCheck } from '../../base/check.js';
import b4a from 'b4a';
import { jsonStringify } from '../../utils/types.js';

function serializableResult(value) {
    if(value === undefined) return null;
    if(value instanceof Error) {
        return {
            name: value.name,
            message: value.message,
        };
    }
    try {
        JSON.stringify(value);
        return value;
    } catch {
        return String(value);
    }
}

function featureResultRecord(op, result, err) {
    const dispatch = op.value.dispatch;
    const ok = result !== undefined && err === null;
    return {
        type: 'feature_result',
        feature_key: op.key,
        hash: dispatch.hash,
        address: dispatch.address ?? null,
        status: ok ? 'applied' : 'rejected',
        ok,
        result: ok ? serializableResult(result) : null,
        error: ok ? null : {
            message: err?.message ?? 'Feature returned no result.',
            name: err?.name ?? 'FeatureRejected',
        },
    };
}

export class FeatureOperation {
    #validator
    #wallet
    #protocolInstance
    #contractInstance

    constructor(validator, { wallet, protocolInstance, contractInstance }) {
        this.#validator = validator
        this.#wallet = wallet
        this.#protocolInstance = protocolInstance
        this.#contractInstance = contractInstance
    }
    async handle(op, batch, base, node) {
        if(false === this.#validator.validateNode(node)) return;
        // Feature apply: signer-signed feature/contract op (replay-protected by sh/<hash>).
        if(b4a.byteLength(jsonStringify(op)) > this.#protocolInstance.featMaxBytes()) return;
        if(false === this.#validator.validate(op)) return;
        const dispatch = op.value.dispatch;
        const strDispatchValue = jsonStringify(dispatch.value);
        if(null === await batch.get(`sh/${dispatch.hash}`)){
            const verified = this.#wallet.verify(dispatch.hash, `${strDispatchValue}${dispatch.nonce}`, dispatch.address);
            if(true === verified) {
                const result = await this.#contractInstance.execute(op, batch);
                const err = this.#protocolInstance.getError(result);
                await batch.put(`fr/${dispatch.hash}`, featureResultRecord(op, result, err));
                if(undefined !== result && null === err) {
                    await batch.put(`sh/${dispatch.hash}`, '');
                }
                //console.log(`Feature ${op.key} appended`);
            }
        }
    }
}

export class FeatureCheck extends BaseCheck {
    #validate

    constructor() {
        super()
        this.#validate = this.#compile()
    }

    #compile() {
        const schema = {
            key: { type : "string", min : 1, max : 256 },
            value : {
                $$type: "object",
                dispatch : {
                    $$type : "object",
                    value : { type : "any", nullable : true },
                    nonce: { type : "string", min : 1, max : 256 },
                    hash: { type : "is_hex" },
                    address: { type : "is_hex" }
                }
            }
        };

        return this.validator.compile(schema)
    }

    validate(op) {
        return this.#validate(op) === true
    }
}
