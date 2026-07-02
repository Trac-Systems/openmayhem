import { BaseCheck } from './base/check.js';

class Check extends BaseCheck {
    constructor() {
        super();

        this.enter_execute_schema = this.validator.compile({
            value : {
                $$type: "object",
                dispatch : {
                    $$type : "object",
                    type : { type : "string", min : 1, max : 256 }
                }
            }
        });

        this.tx_schema = this.validator.compile({
            key : { type : "is_hex" },
            value : {
                $$type: "object",
                dispatch : {
                    $$type : "object",
                    type : { type : "string", min : 1, max : 256 },
                    value : { type : "any", nullable : true }
                },
                ipk : { type : "is_hex" },
                wp : { type : "is_hex" }
            }
        });

        this.address_schema = this.validator.compile({
            value : {
                $$type: "object",
                dispatch : {
                    $$type : "object",
                    address : { type : "is_hex" }
                }
            }
        });

        this.textkey_schema = this.validator.compile({
            key : { type : "string", min : 1, max : 256 }
        });

        this.schemata = {};
    }

    compile(schema) {
        return this.validator.compile(schema);
    }

    validate(compiled, payload) {
        const res = compiled(payload);
        return res === true;
    }

    addSchema(type, schema) {
        this.schemata[type] = this.compile(schema);
    }

    hasSchema(type) {
        return this.schemata[type] !== undefined;
    }

    validateSchema(type, op) {
        return this.validate(this.schemata[type], op);
    }

    validateEnterExecute(op) {
        const res = this.enter_execute_schema(op);
        return res === true;
    }

    validateTx(op) {
        const res = this.tx_schema(op);
        return res === true;
    }

    validateAddress(op) {
        const res = this.address_schema(op);
        return res === true;
    }

    validateTextKey(op) {
        const res = this.textkey_schema(op);
        return res === true;
    }
}

export default Check;
