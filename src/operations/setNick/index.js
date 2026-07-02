import { BaseCheck } from '../../base/check.js';
import b4a from 'b4a';
import { jsonStringify, visibleLength } from '../../utils/types.js';

export class SetNickOperation {
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
        // Chat apply: nickname changes (user/mod/admin-signed, uniqueness-enforced).
        if(false === this.#validator.validate(op)) return;
        const taken = await batch.get(`kcin/${op.value.dispatch.nick}`);
        const chatStatus = await batch.get('chat_status');
        const strValue = jsonStringify(op.value);
        const admin = await batch.get('admin');
        const mod = await batch.get(`mod/${op.value.dispatch.initiator}`);
        let adminVerified = false;
        if(null !== admin) {
            adminVerified = this.#wallet.verify(op.hash, `${strValue}${op.nonce}`, admin.value);
        }
        let modVerified = false;
        if(null !== mod && true === mod.value && op.value.dispatch.address !== op.value.dispatch.initiator) {
            const targetMod = await batch.get(`mod/${op.value.dispatch.address}`);
            if((null === targetMod || false === targetMod.value) && (null === admin || admin.value !== op.value.dispatch.address)){
                modVerified = this.#wallet.verify(op.hash, `${strValue}${op.nonce}`, op.value.dispatch.initiator);
            }
        }
        const verified = this.#wallet.verify(op.hash, `${strValue}${op.nonce}`, op.value.dispatch.address);
        if(null === taken &&
            null !== strValue &&
            ( true === verified || true === modVerified || true === adminVerified ) &&
            null !== chatStatus &&
            chatStatus.value === 'on' &&
            null === await batch.get(`sh/${op.hash}`) &&
            b4a.byteLength(strValue) <= 256 &&
            visibleLength(op.value.dispatch.nick) <= 32){
            const old = await batch.get(`nick/${op.value.dispatch.address}`);
            if(old !== null){
                await batch.del(`nick/${op.value.dispatch.address}`);
                await batch.del(`kcin/${old.value}`);
            }
            await batch.put(`nick/${op.value.dispatch.address}`, op.value.dispatch.nick);
            await batch.put(`kcin/${op.value.dispatch.nick}`, op.value.dispatch.address);
            await batch.put(`sh/${op.hash}`, '');
            console.log(`Changed nick to ${op.value.dispatch.nick} (${op.value.dispatch.address})`);
        }
    }
}

export class SetNickCheck extends BaseCheck {
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
                    nick : { type : "string", min : 1, max : 256 },
                    type : { type : "string", min : 1, max : 256 },
                    address : { type : "is_hex" },
                    initiator : { type : "is_hex" }
                }
            }
        };

        return this.validator.compile(schema)
    }

    validate(op) {
        return this.#validate(op) === true
    }
}
