import { BaseCheck } from '../../base/check.js';
import { jsonStringify } from '../../utils/types.js';

export class SetChatStatusOperation {
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
        // Chat config apply: admin-signed chat on/off toggle.
        if(false === this.#validator.validate(op)) return;
        const strMsg = jsonStringify(op.value.msg);
        const admin = await batch.get('admin');
        if(null !== admin && op.value.msg.key === op.key && op.value.msg.type === 'setChatStatus' && (op.key === 'on' || op.key === 'off') && null === await batch.get(`sh/${op.hash}`)) {
            const verified = this.#wallet.verify(op.hash, `${strMsg}${op.nonce}`, admin.value);
            if(true === verified){
                await batch.put('chat_status', op.key);
                await batch.put(`sh/${op.hash}`, '');
                console.log(`Set chat_status: ${op.key}`);
            }
        }
    }
}

export class SetChatStatusCheck extends BaseCheck {
    #validate

    constructor() {
        super()
        this.#validate = this.#compile()
    }

    #compile() {
        const schema = {
            key: { type : "string", min : 1, max : 256 },
            hash : { type : "is_hex" },
            nonce : { type : "string", min : 1, max : 256 },
            value : {
                $$type: "object",
                msg : {
                    $$type : "object",
                    type : { type : "string", min : 1, max : 256 },
                    key: { type : "string", min : 1, max : 256 }
                }
            }
        };

        return this.validator.compile(schema)
    }

    validate(op) {
        return this.#validate(op) === true
    }
}
