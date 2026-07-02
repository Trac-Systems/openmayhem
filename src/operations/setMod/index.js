import { BaseCheck } from '../../base/check.js';
import { jsonStringify } from '../../utils/types.js';

export class SetModOperation {
    #validator
    #wallet

    constructor(validator, { wallet }) {
        this.#validator = validator
        this.#wallet = wallet
    }
    async handle(op, batch, base, node) {
        if(false === this.#validator.validateNode(node)) return;
        // Chat moderation apply: admin-signed set/unset mod role.
        if(false === this.#validator.validate(op)) return;
        const admin = await batch.get('admin');
        const strValue = jsonStringify(op.value);
        if(null !== admin && null !== strValue &&
            null === await batch.get(`sh/${op.hash}`)){
            const verified = this.#wallet.verify(op.hash, `${strValue}${op.nonce}`, admin.value);
            if(true === verified) {
                await batch.put(`mod/${op.value.dispatch.user}`, op.value.dispatch.mod);
                await batch.put(`sh/${op.hash}`, '');
                console.log(`Changed mod status ${op.value.dispatch.user} to ${op.value.dispatch.mod}`);
            }
        }
    }
}

export class SetModCheck extends BaseCheck {
    #validate

    constructor() {
        super()
        this.#validate = this.#compile()
    }

    #compile() {
        const schema = {
            nonce: { type : "string", min : 1, max : 256 },
            hash: { type : "is_hex" },
            value : {
                $$type: "object",
                dispatch : {
                    $$type : "object",
                    mod : { type : "boolean" },
                    type : { type : "string", min : 1, max : 256 },
                    address : { type : "is_hex" },
                    user : { type : "is_hex" }
                }
            }
        };

        return this.validator.compile(schema)
    }

    validate(op) {
        return this.#validate(op) === true
    }
}
