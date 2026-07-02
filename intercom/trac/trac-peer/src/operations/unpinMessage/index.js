import { BaseCheck } from '../../base/check.js';
import { jsonStringify } from '../../utils/types.js';

export class UnpinMessageOperation {
    #validator
    #wallet

    constructor(validator, { wallet }) {
        this.#validator = validator
        this.#wallet = wallet
    }
    async handle(op, batch, base, node) {
        if(false === this.#validator.validateNode(node)) return;
        // Chat moderation apply: admin/mod-signed unpin.
        if(false === this.#validator.validate(op)) return;
        const strValue = jsonStringify(op.value);
        if(null !== strValue &&
            null === await batch.get(`sh/${op.hash}`)){
            const mod = await batch.get(`mod/${op.value.dispatch.address}`);
            const pin = await batch.get(`pni/${op.value.dispatch.id}`);
            if(null !== pin) {
                const admin = await batch.get('admin');
                let modVerified = false;
                if(null !== mod && true === mod.value) {
                    modVerified = this.#wallet.verify(op.hash, `${strValue}${op.nonce}`, op.value.dispatch.address);
                }
                const verified = this.#wallet.verify(op.hash, `${strValue}${op.nonce}`, admin.value);
                if(true === verified || true === modVerified) {
                    const message = await batch.get(`msg/${pin.value.msg}`)
                    if(null !== message){
                        message.value.pinned = false;
                        message.value.pin_id = null;
                        await batch.put(`msg/${pin.value.msg}`, message.value);
                        await batch.put(`pni/${op.value.dispatch.id}`, { msg : pin.value.msg, pinned : false });
                    }
                    await batch.put(`sh/${op.hash}`, '');
                    console.log(`Unpinned message ${pin.value.msg} by ${op.value.dispatch.address}`);
                }
            }
        }
    }
}

export class UnpinMessageCheck extends BaseCheck {
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
                    id : { type : "number", integer: true, min : 0, max : Number.MAX_SAFE_INTEGER },
                    type : { type : "string", min : 1, max : 256 },
                    address : { type : "is_hex" }
                }
            }
        };

        return this.validator.compile(schema)
    }

    validate(op) {
        return this.#validate(op) === true
    }
}
