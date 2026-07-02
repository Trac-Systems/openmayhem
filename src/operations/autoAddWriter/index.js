import { BaseCheck } from '../../base/check.js';
import b4a from 'b4a';

export class AutoAddWritersOperation {
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
        if(false === this.#validator.validate(op)) return;
        const autoAddWriters = await batch.get('auto_add_writers');
        const banned = await batch.get(`bnd/${op.key}`);
        if(null === banned && null !== autoAddWriters && autoAddWriters.value === 'on'){
            const writerKey = b4a.from(op.key, 'hex');
            await base.addWriter(writerKey, { isIndexer : false });
        }
        console.log(`Writer auto added: ${op.key}`);
    }
}

export class AutoAddWritersCheck extends BaseCheck {
    #validate

    constructor() {
        super()
        this.#validate = this.#compile()
    }

    #compile() {
        const schema = {
            key: { type : "is_hex" }
        };

        return this.validator.compile(schema)
    }

    validate(op) {
        return this.#validate(op) === true
    }
}
