import Validator from 'fastest-validator';
import WAValidator from 'multicoin-address-validator';
import b4a from 'b4a';

export class BaseCheck {
    #validateNode

    constructor() {
        this.validator = new Validator({
            useNewCustomCheckerFunction: true,
            messages: {
                bufferedHex: "The '{field}' field must a hex! Actual: {actual}",
                bigint: "The '{field}' field must a biginteger or a biginteger string! Actual: {actual}",
                bitcoin: "The '{field}' field must a valid Bitcoin address! Actual: {actual}"
            },
            customFunctions : {
                bitcoinAddress : (value, errors)=>{
                    let result = false
                    let result2 = false
                    let result3 = false
                    let result4 = false
                    let result5 = false
                    try{
                        result = WAValidator.validate(value, 'Bitcoin', { networkType : 'both' });
                        if(false === result){
                            result2 = WAValidator.validate(value, 'Solana', { networkType : 'both' });
                            if(false === result2){
                                result3 = WAValidator.validate(value, 'DogeCoin', { networkType : 'both' });
                                if(false === result3){
                                    result4 = WAValidator.validate(value, 'BinanceSmartChain', { networkType : 'both' });
                                    if(false === result4){
                                        // check for Trac public key hex
                                        let buf = null
                                        try{
                                            buf = b4a.from(value, 'hex')
                                            result5 = value === b4a.toString(buf, 'hex')
                                            if(result5){
                                                result5 = value.length === 64
                                            }
                                        } catch (e) {}
                                    }
                                }
                            }
                        }
                    } catch (e) {}
                    return result || result2 || result3 || result4 || result5;
                },
                hexCheck : (value, errors) => {
                    let buf = null
                    let result = false
                    try{
                        buf = b4a.from(value, 'hex')
                        result = value === b4a.toString(buf, 'hex')
                    } catch (e) {}
                    return result;
                }
            }
        });

        this.validator.add("bigint", function({ schema, messages }, path, context) {
            return {
                source: `
                    let result = false
                    try{ 
                        BigInt(value);
                    } catch (e) {}
                    if (false === result)
                        ${this.makeError({ type: "bigint",  actual: "value", messages })}
                    return value
                `
            };
        });

        this.validator.add("is_hex", function({ schema, messages }, path, context) {
            return {
                source: `
                    const result = context.customFunctions.hexCheck(value, errors);
                    if(false === result) ${this.makeError({ type: "bufferedHex",  actual: "value", messages })}
                    return value;
                `
            };
        });

        this.validator.add("bitcoin_address", function({ schema, messages }, path, context) {
            return {
                source: `
                    const result = context.customFunctions.bitcoinAddress(value, errors);
                    if(false === result) ${this.makeError({ type: "bitcoin",  actual: "value", messages })}
                    return value;
                `
            };
        });

        this.#validateNode = this.#compileNode();
    }

    #compileNode() {
        const schema = {
            /*from : {
                $$type: "object",
                key : { type : 'string', hex : null, instanceof: Buffer }
            },*/
            value: {
                $$type: "object",
                type: { type : "string", min : 1, max : 256 }
            }
        };
        return this.validator.compile(schema);
    }

    validateNode(node) {
        const res = this.#validateNode(node);
        return res === true;
    }
}
