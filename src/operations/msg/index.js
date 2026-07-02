import { BaseCheck } from '../../base/check.js';
import b4a from 'b4a';
import { jsonStringify } from '../../utils/types.js';

export class MsgOperation {
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
        // Chat apply: user-signed message + whitelist/mute checks + replay protection.
        if (b4a.byteLength(jsonStringify(op)) > this.#protocolInstance.msgMaxBytes()) return;
        if (false === this.#validator.validate(op)) return;
        const admin = await batch.get('admin');
        let muted = false;
        let whitelisted = true;
        const whitelistStatus = await batch.get('wlst');
        if (null !== whitelistStatus && true === whitelistStatus.value) {
            const whitelistedEntry = await batch.get(`wl/${op.value.dispatch.address}`);
            if (null === whitelistedEntry || false === whitelistedEntry.value) {
                whitelisted = false;
            }
        }
        const muteStatus = await batch.get(`mtd/${op.value.dispatch.address}`);
        if (null !== muteStatus) {
            muted = muteStatus.value;
        }
        if (null !== admin && admin.value === op.value.dispatch.address) {
            muted = false;
            whitelisted = true;
        }
        const strValue = jsonStringify(op.value);
        const chatStatus = await batch.get('chat_status');
        const verified = this.#wallet.verify(op.hash, `${strValue}${op.nonce}`, op.value.dispatch.address);

        if (true === verified && false === muted && true === whitelisted && null !== strValue && null !== chatStatus && null === await batch.get(`sh/${op.hash}`) && chatStatus.value === 'on' && null === this.#protocolInstance.getError(await this.#contractInstance.execute(op, batch))) {
            let len = await batch.get('msgl');
            if (null === len) {
                len = 0;
            } else {
                len = len.value;
            }

            let userLen = await batch.get(`umsgl/${op.value.dispatch.address}`);
            if (null === userLen) {
                userLen = 0;
            } else {
                userLen = userLen.value;
            }

            await batch.put(`msg/${len}`, op.value.dispatch);
            await batch.put(`umsg/${op.value.dispatch.address}/${userLen}`, `msg/${len}`);
            await batch.put('msgl', len + 1);
            await batch.put(`umsgl/${op.value.dispatch.address}`, userLen + 1);
            await batch.put(`sh/${op.hash}`, '');
            const nick = await batch.get(`nick/${op.value.dispatch.address}`);
            console.log(`#${len + 1} | ${nick !== null ? nick.value : op.value.dispatch.address}: ${op.value.dispatch.msg}`);
        }
    }
}

export class MsgCheck extends BaseCheck {
    #validate

    constructor() {
        super()
        this.#validate = this.#compile()
    }

    #compile() {
        const schema = {
            $$strict: true,
            type : { type : "string", min : 1, max : 256 },
            nonce: { type : "string", min : 1, max : 256 },
            hash: { type : "is_hex" },
            value : {
                $$strict: true,
                $$type: "object",
                dispatch : {
                    $$strict: true,
                    $$type : "object",
                    attachments : { type : "array", items : "string" },
                    msg : { type : "string", min : 1 },
                    type : { type : "string", min : 1, max : 256 },
                    address : { type : "is_hex" },
                    deleted_by : { type : "is_hex", nullable : true },
                    reply_to : { type : "number", integer : true, min : 0, max : Number.MAX_SAFE_INTEGER, nullable : true },
                    pinned : { type : "boolean" },
                    pin_id : { type : "number", integer : true, min : 0, max : Number.MAX_SAFE_INTEGER, nullable : true },
                }
            }
        };

        return this.validator.compile(schema)
    }

    validate(op) {
        return this.#validate(op) === true
    }
}
