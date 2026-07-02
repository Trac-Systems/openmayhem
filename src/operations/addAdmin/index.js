import { BaseCheck } from '../../base/check.js';
import b4a from 'b4a';

export class AddAdminOperation {
    #config
    #validator

    constructor(validator, { config }) {
        this.#validator = validator
        this.#config = config
    }
    async handle(op, batch, base, node) {
        if(false === this.#validator.validateNode(node)) return;
        // Admin apply: bootstrap node can set the initial admin once.
        if(false === this.#validator.validate(op)) return;
        const bootstrapWriterKeyHex = b4a.toString(node.from.key, 'hex');
        const subnetBootstrapHex = (b4a.isBuffer(this.#config.bootstrap) ? this.#config.bootstrap.toString('hex') : `${this.#config.bootstrap}`).toLowerCase();
        if(null === await batch.get('admin') && bootstrapWriterKeyHex === subnetBootstrapHex){
            await batch.put('admin', op.key);
            console.log(`Admin added: ${op.key}`);
        }
    }
}

export class AddAdminCheck extends BaseCheck {
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
