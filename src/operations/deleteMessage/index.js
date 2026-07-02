import { BaseCheck } from '../../base/check.js';
import { jsonStringify } from '../../utils/types.js';

export class DeleteMessageOperation {
    #validator
    #wallet

    constructor(validator, { wallet }) {
        this.#validator = validator
        this.#wallet = wallet
    }
    async handle(op, batch, base, node) {
        if(false === this.#validator.validateNode(node)) return;
        // Chat moderation apply: admin/mod/user-signed message deletion (replay-protected by sh/<hash>).
        if(false === this.#validator.validate(op)) return;
        const strValue = jsonStringify(op.value);
        if(null !== strValue &&
            null === await batch.get(`sh/${op.hash}`)){
            const mod = await batch.get(`mod/${op.value.dispatch.address}`);
            const message = await batch.get(`msg/${op.value.dispatch.id}`);
            if(null !== message && null !== message.value && message.value.deleted_by !== undefined && null === message.value.deleted_by) {
                const admin = await batch.get('admin');
                let modVerified = false;
                if(null !== mod && true === mod.value && message.value.address !== admin.value) {
                    modVerified = this.#wallet.verify(op.hash, `${strValue}${op.nonce}`, op.value.dispatch.address);
                }
                let userVerified = false;
                if((null === mod || false === mod.value) && message.value.address === op.value.dispatch.address &&
                    op.value.dispatch.address !== admin.value) {
                    userVerified = this.#wallet.verify(op.hash, `${strValue}${op.nonce}`, op.value.dispatch.address);
                }
                const verified = this.#wallet.verify(op.hash, `${strValue}${op.nonce}`, admin.value);
                if(true === verified || true === modVerified || true === userVerified) {
                    message.value.msg = null;
                    message.value.attachments = [];
                    message.value.deleted_by = verified ? admin.value : op.value.dispatch.address;
                    let len = await batch.get('delml');
                    if(null === len) {
                        len = 0;
                    } else {
                        len = len.value;
                    }
                    await batch.put(`msg/${op.value.dispatch.id}`, message.value);
                    await batch.put(`delm/${len}`, op.value.dispatch.id);
                    await batch.put('delml', len + 1);
                    await batch.put(`sh/${op.hash}`, '');
                    console.log(`Deleted message ${op.value.dispatch.id} of user ${message.value.address} by ${op.value.dispatch.address}`);
                }
            }
        }
    }
}

export class DeleteMessageCheck extends BaseCheck {
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
