import { formatNumberString, resolveNumberString, jsonStringify, jsonParse, safeClone, createHash } from "../utils/types.js";
import {ProtocolApi} from '../api.js';
import Wallet from 'trac-wallet';
import b4a from 'b4a';
import { createMessage } from 'trac-msb/src/utils/buffer.js';
import { blake3 } from '@tracsystems/blake3'
import { MSB_OPERATION_TYPE } from '../msbClient.js';

class Protocol{
    constructor(peer, base, config) {
        this.api = new ProtocolApi(peer, config);
        this.base = base;
        this.peer = peer;
        this.config = config;
        this.input = null;
        this.tokenized_input = null;
        this.fromBigIntString = formatNumberString;
        this.toBigIntString = resolveNumberString;
        this.safeJsonStringify = jsonStringify;
        this.safeJsonParse = jsonParse;
        this.safeClone = safeClone;
        this.features = {};
    }

    getApiSchema() {
        const api = this.api;
        if (!api) return { methods: {} };

        const hex32 = { type: "string", pattern: "^[0-9a-fA-F]{64}$" };
        const hex64 = { type: "string", pattern: "^[0-9a-fA-F]{128}$" };
        const preparedCommand = {
            type: "object",
            additionalProperties: true,
            properties: {
                type: { type: "string", minLength: 1, maxLength: 256 },
                value: {},
            },
            required: ["type", "value"],
        };

        const baseMethods = {
            generateNonce: { params: [], returns: { type: "string", minLength: 1, maxLength: 256 } },
            prepareTxCommand: { params: [{ name: "command", schema: { type: "string" } }], returns: preparedCommand },
            generateTx: {
                params: [
                    { name: "address", schema: hex32 },
                    { name: "command_hash", schema: hex32 },
                    { name: "nonce", schema: hex32 },
                ],
                returns: hex32,
            },
            tx: {
                params: [
                    { name: "tx", schema: hex32 },
                    { name: "prepared_command", schema: preparedCommand },
                    { name: "address", schema: hex32 },
                    { name: "signature", schema: hex64 },
                    { name: "nonce", schema: hex32 },
                    { name: "sim", schema: { type: "boolean" } },
                ],
                returns: {},
            },
        };

        const methods = {};
        for (const [name, def] of Object.entries(baseMethods)) {
            if (typeof api[name] === "function") methods[name] = def;
        }

        // Include extendApi() methods (own props) - treated as read/query methods by convention.
        for (const name of Object.getOwnPropertyNames(api)) {
            if (name === "constructor") continue;
            if (name.startsWith("_")) continue;
            if (methods[name] !== undefined) continue;
            if (typeof api[name] !== "function") continue;

            const params = [];
            const count = Number.isSafeInteger(api[name].length) ? api[name].length : 0;
            for (let i = 0; i < count; i++) params.push({ name: `arg${i}`, schema: {} });
            methods[name] = { params, returns: {} };
        }

        return { methods };
    }

    featMaxBytes(){
        return 4_096;
    }

    txMaxBytes(){
        return 4_096;
    }

    msgMaxBytes(){
        return 8_192;
    }

    generateNonce(){
        return Wallet.generateNonce().toString('hex');
    }

    safeBigInt(value) {
        try{
            return BigInt(value);
        } catch(e) { }
        return null;
    }

    parseArgs(cmdline) {
        let re_next_arg = /^\s*((?:(?:"(?:\\.|[^"])*")|(?:'[^']*')|\\.|\S)+)\s*(.*)$/;
        let next_arg = ['', '', cmdline];
        let args = [];
        const obj = {};
        while (next_arg = re_next_arg.exec(next_arg[2])) {
            let quoted_arg = next_arg[1];
            let unquoted_arg = "";
            while (quoted_arg.length > 0) {
                if (/^"/.test(quoted_arg)) {
                    let quoted_part = /^"((?:\\.|[^"])*)"(.*)$/.exec(quoted_arg);
                    unquoted_arg += quoted_part[1].replace(/\\(.)/g, "$1");
                    quoted_arg = quoted_part[2];
                } else if (/^'/.test(quoted_arg)) {
                    let quoted_part = /^'([^']*)'(.*)$/.exec(quoted_arg);
                    unquoted_arg += quoted_part[1];
                    quoted_arg = quoted_part[2];
                } else if (/^\\/.test(quoted_arg)) {
                    unquoted_arg += quoted_arg[1];
                    quoted_arg = quoted_arg.substring(2);
                } else {
                    unquoted_arg += quoted_arg[0];
                    quoted_arg = quoted_arg.substring(1);
                }
            }

            args[args.length] = unquoted_arg;
        }
        args = args.splice(1);
        for(let i = 0; i < args.length; i++){
            if(i % 2 === 0) {
                if(args[i].startsWith('--')) args[i] = args[i].substring(2);
                obj[args[i]] = args[i+1] !== undefined ? args[i+1] : null;
            }
        }
        return obj;
    }

    async addFeature(key, feature){
        feature.key = key;
        if(typeof this.features[key] !== "undefined") key = Math.random();
        this.features[key] = feature;
    }

    async generateTx(networkId, txvHex, incomingWritingKeyHex, contentHashHex, externalBootstrapHex, msbBootstrapHex, nonceHex){
        const msg = createMessage(
            networkId,
            b4a.from(txvHex, 'hex'),
            b4a.from(incomingWritingKeyHex, 'hex'),
            b4a.from(contentHashHex, 'hex'),
            b4a.from(externalBootstrapHex, 'hex'),
            b4a.from(msbBootstrapHex, 'hex'),
            b4a.from(nonceHex, 'hex'),
            MSB_OPERATION_TYPE.TX
        );
        const tx = await blake3(msg);
        return b4a.toString(tx, 'hex');
    }

    async simulateTransaction(validator_pub_key, obj, surrogate = null){
        const storage = new SimStorage(this.peer);
        let nonce = this.generateNonce();
        const content_hash = await createHash(this.safeJsonStringify(obj));
        const txvHex = await this.peer.msbClient.getTxvHex();
        const msbBootstrapHex = this.peer.msbClient.bootstrapHex;
        const subnetBootstrapHex = (b4a.isBuffer(this.peer.config.bootstrap) ? this.peer.config.bootstrap.toString('hex') : (''+this.peer.config.bootstrap)).toLowerCase();
        const tx = await this.generateTx(
            this.peer.msbClient.networkId,
            txvHex,
            this.peer.writerLocalKey,
            content_hash,
            subnetBootstrapHex,
            msbBootstrapHex,
            nonce
        );
        const op = {
            type : 'tx',
            key : tx,
            value : {
                dispatch : obj,
                ipk : surrogate !== null ? surrogate.address : this.peer.wallet.publicKey,
                wp : validator_pub_key
            }
        }
        return await this.peer.contract.instance.execute(op, storage);
    }

    async broadcastTransaction(obj, sim = false, surrogate = null){
        const tx_enabled = await this.peer.base.view.get('txen');
        // Default to enabled if missing, consistent with apply() gating.
        if (null !== tx_enabled && true !== tx_enabled.value ) throw new Error('Tx is not enabled.');
        if(this.peer.wallet.publicKey === null || this.peer.wallet.secretKey === null) throw new Error('Wallet is not initialized.');
        if(this.peer.writerLocalKey === null) throw new Error('Local writer is not initialized.');
        if(obj.type === undefined || obj.value === undefined) throw new Error('Invalid transaction object.');

        const validator_pub_key = '0'.repeat(64);
        if(true === sim) {
            return await this.simulateTransaction(validator_pub_key, obj, surrogate);
        }

        const txvHex = await this.peer.msbClient.getTxvHex();
        const msbBootstrapHex = this.peer.msbClient.bootstrapHex;
        const subnetBootstrapHex = (b4a.isBuffer(this.peer.config.bootstrap) ? this.peer.config.bootstrap.toString('hex') : (''+this.peer.config.bootstrap)).toLowerCase();
        const content_hash = await createHash(this.safeJsonStringify(obj));

        let nonceHex, txHex, signatureHex, pubKeyHex;
        if(surrogate !== null) {
            nonceHex = surrogate.nonce;
            txHex = surrogate.tx;
            signatureHex = surrogate.signature;
            pubKeyHex = surrogate.address;
        } else {
            nonceHex = this.generateNonce();
            txHex = await this.generateTx(
                this.peer.msbClient.networkId,
                txvHex,
                this.peer.writerLocalKey,
                content_hash,
                subnetBootstrapHex,
                msbBootstrapHex,
                nonceHex
            );
            signatureHex = this.peer.wallet.sign(b4a.from(txHex, 'hex'));
            pubKeyHex = this.peer.wallet.publicKey;
        }

        const address = this.peer.msbClient.pubKeyHexToAddress(pubKeyHex);
        if(address === null) throw new Error('Failed to create MSB address from public key.');

        const payload = {
            type: MSB_OPERATION_TYPE.TX,
            address: address,
            txo: {
                tx: txHex,
                txv: txvHex,
                iw: this.peer.writerLocalKey,
                in: nonceHex,
                ch: content_hash,
                is: signatureHex,
                bs: subnetBootstrapHex,
                mbs: msbBootstrapHex
            }
        };

        await this.peer.msbClient.broadcastTransaction(payload);
        if(this.peer.txPool.isNotFull() && !this.peer.txPool.contains(txHex)){
            this.peer.txPool.add(txHex, { dispatch : obj, ipk : pubKeyHex, address : address });
        }
        return payload;
    }

    async tokenizeInput(input){
        this.input = input;
        if(typeof input === "string"){
            this.tokenized_input = input.split(' ').map(function(item) {
                return item.trim();
            });
        }
    }

    getError(value){
        if (value === false) return new Error('Error');
        if (value === null || value === undefined) return null;
        if (typeof value === 'object' && value.stack !== undefined && value.message !== undefined) {
            return value;
        }
        return null;
    }

    mapTxCommand(command){
        return null;
    }

    async tx(subject, sim = false, surrogate = null){
        const obj = this.mapTxCommand(subject.command);
        if(null !== obj && typeof obj.type === 'string' && obj.value !== undefined) {
            return await this.broadcastTransaction({
                type : obj.type,
                value : obj.value
            }, sim, surrogate);
            const _this = this;
            async function push(){
                await _this.peer.sleep(10_000);
                try{
                    await _this.broadcastTransaction({
                        type : 'p',
                        value : ''
                    });
                } catch(e) { }
            }
            push();
            return ret;
        }
        throw new Error('HyperMallProtocol::tx(): command not found.');
    }

    async customCommand(input){ }

    async printOptions(){ }

    async extendApi(){ }

    async getSigned(key){
        const view_session = this.peer.base.view.checkout(this.peer.base.view.core.signedLength);
        const result = await view_session.get(key);
        await view_session.close();
        if(result === null) return null;
        return result.value;
    }

    async get(key){
        const result = await this.peer.base.view.get(key);
        if(null === result) return null;
        return result.value;
    }
}

class SimStorage{
    constructor(peer) {
        this.peer = peer;
        this.values = {};
    }

    async del(key){
        if(this.peer.contract.instance.isReservedKey(key)) throw new Error('del(key): ' + key + 'is reserved');
        delete this.values[key];
        return this.peer.contract.instance.emptyPromise();
    }

    async get(key){
        if(this.values[key] !== undefined) {
            return { value : this.values[key] };
        }
        return await this.peer.base.view.get(key);
    }

    async put(key, value){
        if(this.peer.contract.instance.isReservedKey(key)) throw new Error('put(key,value): ' + key + 'is reserved');
        this.values[key] = value;
        return this.peer.contract.instance.emptyPromise();
    }
}

export default Protocol;
