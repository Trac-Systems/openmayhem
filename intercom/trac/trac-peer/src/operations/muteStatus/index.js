import { BaseCheck } from '../../base/check.js';
import { jsonStringify } from '../../utils/types.js';

export class MuteStatusOperation {
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
        // Chat moderation apply: admin/mod-signed mute/unmute (stored under mtd/<user>).
        if(false === this.#validator.validate(op)) return;
        const admin = await batch.get('admin');
        const strValue = jsonStringify(op.value);
        if(null !== strValue &&
            null === await batch.get(`sh/${op.hash}`)){
            const mod = await batch.get(`mod/${op.value.dispatch.address}`);
            let modVerified = false;
            if(null !== mod && true === mod.value && admin.value !== op.value.dispatch.user) {
                const targetMod = await batch.get(`mod/${op.value.dispatch.user}`);
                if(null === targetMod || false === targetMod.value) {
                    modVerified = this.#wallet.verify(op.hash, `${strValue}${op.nonce}`, op.value.dispatch.address);
                }
            }
            const verified = this.#wallet.verify(op.hash, `${strValue}${op.nonce}`, admin.value);
            if(true === verified || true === modVerified) {
                await batch.put(`mtd/${op.value.dispatch.user}`, op.value.dispatch.muted);
                await batch.put(`sh/${op.hash}`, '');
                console.log(`Changed mute status ${op.value.dispatch.user} to ${op.value.dispatch.muted}`);
            }
        }
    }
}

export class MuteStatusCheck extends BaseCheck {
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
                    muted : { type : "boolean" },
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
