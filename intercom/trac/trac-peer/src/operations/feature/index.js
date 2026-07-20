import { BaseCheck } from '../../base/check.js';
import b4a from 'b4a';
import { jsonStringify } from '../../utils/types.js';

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
        // Feature apply: admin-signed feature/contract op (replay-protected by sh/<hash>).
        if(b4a.byteLength(jsonStringify(op)) > this.#protocolInstance.featMaxBytes()) return;
        if(false === this.#validator.validate(op)) return;
        const strDispatchValue = jsonStringify(op.value.dispatch.value);
        const admin = await batch.get('admin');
        if(null !== admin &&
            null === await batch.get(`sh/${op.value.dispatch.hash}`)){
            const verified = this.#wallet.verify(op.value.dispatch.hash, `${strDispatchValue}${op.value.dispatch.nonce}`, admin.value);
            if(true === verified) {
                await this.#contractInstance.execute(op, batch);
                await batch.put(`sh/${op.value.dispatch.hash}`, '');
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
                    hash: { type : "is_hex" }
                }
            }
        };

        return this.validator.compile(schema)
    }

    validate(op) {
        return this.#validate(op) === true
    }
}
