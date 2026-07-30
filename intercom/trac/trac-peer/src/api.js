import b4a from "b4a";
import { jsonStringify, createHash } from "./utils/types.js";

export class ProtocolApi {

    /**
     * Exposes read and write functions.
     * May be extended by contract protocol instances.
     *
     * @param peer
     * @param config
     */
    constructor(peer, config) {
        this.peer = peer;
        this.apiTxExposed = config.apiTxExposed === true;
        this.apiMsgExposed = config.apiMsgExposed === true;
    }

    /**
     *
     * @returns {null|string}
     */
    getPeerValidatorAddress(){
        return null;
    }

    /**
     *
     * @returns {null|string}
     */
    getPeerBootstrap(){
        return this.peer.config.bootstrap;
    }

    /**
     *
     * @returns {null|string}
     */
    getPeerMsbBootstrap(){
        return this.peer.msbClient.bootstrapHex;
    }

    /**
     *
     * @returns {null|string}
     */
    getPeerWriterKey(){
        return this.peer.writerLocalKey;
    }

    /**
     *
     * @returns {string}
     */
    generateNonce(){
        return this.peer.protocol.instance.generateNonce();
    }

    /**
     * @throws Error
     * @param msg
     * @param address
     * @param reply_to
     * @param attachments
     * @returns {{dispatch: {type: string, msg: string, address, attachments: *[], deleted_by: null, reply_to: (number|null), pinned: boolean, pin_id: null}}}
     */
    prepareMessage(msg, address, reply_to = null, attachments = []){
        if(typeof msg !== 'string') throw new Error('Msg must be a string');
        if(b4a.byteLength(jsonStringify(address)) !== 66) throw new Error('Address too large.');
        if(reply_to !== null && isNaN(parseInt(reply_to))) throw new Error('Reply to not a number.');
        if(false === Array.isArray(attachments)) throw new Error('attachments must be an array.');
        if(attachments.length > 20) throw new Error('Too many attachments');
        for(let i = 0; i < attachments.length; i++){
            if(typeof attachments[i] !== 'string') throw new Error('Attachment at index ' + i + ' is not a string.');
        }
        const prepared = {
            dispatch : {
                type : 'msg',
                msg: msg,
                address : address,
                attachments : attachments,
                deleted_by : null,
                reply_to : reply_to !== null ? parseInt(reply_to) : null,
                pinned : false,
                pin_id : null
            }};
        if(b4a.byteLength(jsonStringify(prepared)) > this.peer.protocol.instance.msgMaxBytes()) throw new Error('Message too large.');
        return prepared;
    }

    /**
     *
     * @returns {boolean}
     */
    msgExposed(){
        return true === this.apiMsgExposed;
    }

    /**
     * To post a message, an ed25519 signature has to be provided over the given message + nonce.
     *
     * Signing steps:
     *
     * let address = your_ed25519_wallet.getAccountAddress()
     * let nonce = api.generateNonce()
     * let prepared_message = api.prepareMessage("my message", address)
     * let signature = your_ed25519_wallet.sign(JSON.stringify(prepared_message) + nonce)
     * let result = api.post(prepared_message, signature, nonce)
     *
     * Note: new messages can be read from this api's msg functions.
     *
     * @throws Error
     * @param prepared_message
     * @param signature
     * @param nonce
     * @returns {Promise<void>}
     */
    async post(prepared_message, signature, nonce){
        if(true !== this.apiMsgExposed) throw new Error('Posting messages not exposed in API.');
        if(this.peer.base.writable === false) throw new Error('Peer is not writable.');
        if(b4a.byteLength(jsonStringify(prepared_message)) > this.peer.protocol.instance.msgMaxBytes()) throw new Error('Prepared message too large.');
        if(typeof prepared_message !== 'object') throw new Error('Prepared message must be an object generated with api.prepareMessage().');
        if(prepared_message.dispatch === undefined || prepared_message.dispatch.type === undefined ||
            prepared_message.dispatch.msg === undefined || prepared_message.dispatch.address === undefined ||
            prepared_message.dispatch.attachments === undefined || prepared_message.dispatch.deleted_by === undefined ||
            prepared_message.dispatch.reply_to === undefined || prepared_message.dispatch.pinned === undefined ||
            prepared_message.dispatch.pin_id === undefined) throw new Error('Invalid prepared message.');
        if(prepared_message.dispatch.type !== 'msg') throw new Error('Invalid type.');
        if(typeof prepared_message.dispatch.msg !== 'string') throw new Error('Msg must be a string');
        if(b4a.toString(b4a.from(prepared_message.dispatch.address, 'hex'), 'hex') !== prepared_message.dispatch.address) throw new Error('Invalid address.');
        if(b4a.toString(b4a.from(signature, 'hex'), 'hex') !== signature) throw new Error('Invalid signature.');
        if(b4a.toString(b4a.from(nonce, 'hex'), 'hex') !== nonce) throw new Error('Invalid nonce.');
        if(false === Array.isArray(prepared_message.dispatch.attachments)) throw new Error('attachments must be an array.');
        if(prepared_message.dispatch.attachments.length > 20) throw new Error('Too many attachments');
        for(let i = 0; i < prepared_message.dispatch.attachments.length; i++){
            if(typeof prepared_message.dispatch.attachments[i] !== 'string') throw new Error('Attachment at index ' + i + ' is not a string.');
        }
        if(prepared_message.dispatch.deleted_by !== null) throw new Error('deleted_by must be null');
        if(prepared_message.dispatch.reply_to !== null && isNaN(parseInt(prepared_message.dispatch.reply_to))) throw new Error('Reply to not a number.');
        if(prepared_message.dispatch.pinned !== false) throw new Error('pinned must be false');
        if(prepared_message.dispatch.pin_id !== null) throw new Error('pin_id must be null');

        const verified = this.peer.wallet.verify(signature, JSON.stringify(prepared_message) + nonce, prepared_message.dispatch.address);
        if(false === verified) throw new Error('Invalid signature. Please sign your prepared message.');
        await this.peer.base.append({type: 'msg', value: prepared_message, hash : signature, nonce: nonce });
    }

    /**
     * @throws Error
     * @param address
     * @param command_hash
     * @param nonce
     * @returns {Promise<*>}
     */
    async generateTx(address, command_hash, nonce) {
        const txvHex = await this.peer.msbClient.getTxvHex();
        const subnetBootstrapHex = (b4a.isBuffer(this.getPeerBootstrap()) ? this.getPeerBootstrap().toString('hex') : (''+this.getPeerBootstrap())).toLowerCase();
        return await this.peer.protocol.instance.generateTx(
            this.peer.msbClient.networkId,
            txvHex,
            this.getPeerWriterKey(),
            command_hash,
            subnetBootstrapHex,
            this.getPeerMsbBootstrap(),
            nonce
        );
    }

    /**
     *
     * @param command
     * @returns {*}
     */
    prepareTxCommand(command){
        return this.peer.protocol.instance.mapTxCommand(command);
    }

    /**
     *
     * @returns {boolean}
     */
    txExposed(){
        return true === this.apiTxExposed;
    }

    /**
     * To broadcast a TX, an ed25519 signature has to be provided over the given tx + nonce.
     *
     * Signing steps:
     *
     * let address = your_ed25519_wallet.getAccountAddress()
     * let nonce = api.generateNonce()
     * let tx_hash = api.generateTx(address, a_blake3_function(JSON.stringify(api.prepareTxCommand(command))), nonce)
     * let signature = your_ed25519_wallet.sign(tx + nonce)
     * // simulating
     * let sim_result = api.tx(tx_hash, api.prepareTxCommand(command), address, signature, nonce, true)
     * // broadcasting
     * let result = api.tx(tx_hash, api.prepareTxCommand(command), address, signature, nonce)
     *
     * @throws Error
     * @param tx
     * @param prepared_command
     * @param address
     * @param signature
     * @param nonce
     * @param sim
     * @returns {Promise<boolean|object>}
     */
    async tx(tx, prepared_command, address, signature, nonce, sim = false ){
        if(true !== this.apiTxExposed) throw new Error('Transactions not exposed in API.');
        if(this.peer.base.writable === false) throw new Error('Peer is not writable.');
        if(typeof prepared_command !== 'object') throw new Error('prepared_command must be an object.');
        if(typeof prepared_command.type !== 'string') throw new Error('prepared_command.type must exist and be a string.');
        if(prepared_command.value === undefined) throw new Error('prepared_command.value is missing.');
        if(b4a.byteLength(jsonStringify(prepared_command)) > this.peer.protocol.instance.txMaxBytes()) throw new Error('prepared_command too large.');
        if(b4a.byteLength(jsonStringify(address)) !== 66) throw new Error('Address length invalid.');
        if(b4a.byteLength(jsonStringify(signature)) !== 130) throw new Error('Signature length invalid.');
        if(b4a.byteLength(jsonStringify(nonce)) !== 66) throw new Error('Nonce length invalid.');
        if(b4a.toString(b4a.from(address, 'hex'), 'hex') !== address) throw new Error('Invalid address.');
        if(b4a.toString(b4a.from(signature, 'hex'), 'hex') !== signature) throw new Error('Invalid signature.');
        if(b4a.toString(b4a.from(nonce, 'hex'), 'hex') !== nonce) throw new Error('Invalid nonce.');
        const verified = this.peer.wallet.verify(signature, b4a.from(tx, 'hex'), address);
        if(false === verified) throw new Error('Invalid signature.');
        const content_hash = await createHash(this.peer.protocol.instance.safeJsonStringify(prepared_command));
        let _tx = await this.generateTx(address, content_hash, nonce);
        if(tx !== _tx) throw new Error('Invalid TX.');
        const surrogate = { tx : _tx, nonce : ''+nonce, signature : ''+signature, address : ''+address };
        const res = await this.peer.protocol.instance.broadcastTransaction(
            {
                type: prepared_command.type,
                value: prepared_command.value
            },
            sim === true,
            surrogate
        );

        if (sim === true) {
            const err = this.peer.protocol.instance.getError(res);
            if (null !== err) throw err;
        }

        return res;
    }

    /**
     *
     * @param signed
     * @returns {Promise<null>}
     */
    async getAdmin(signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('admin');
        if(false === signed) res = await this.peer.protocol.instance.get('admin');
        if(null !== res) return res;
        return null;
    }

    /**
     *
     * @param signed
     * @returns {Promise<boolean|null>}
     */
    async getWhitelistEnabled(signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('wlst');
        if(false === signed) res = await this.peer.protocol.instance.get('wlst');
        if(null !== res) return res;
        return false;
    }

    /**
     *
     * @param address
     * @param signed
     * @returns {Promise<boolean|null>}
     */
    async getWhitelistStatus(address, signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('wl/'+address);
        if(false === signed) res = await this.peer.protocol.instance.get('wl/'+address);
        if(null !== res) return res;
        return false;
    }

    /**
     *
     * @param address
     * @param signed
     * @returns {Promise<boolean|null>}
     */
    async getModStatus(address, signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('mod/'+address);
        if(false === signed) res = await this.peer.protocol.instance.get('mod/'+address);
        if(null !== res) return res;
        return false;
    }

    /**
     *
     * @param address
     * @param signed
     * @returns {Promise<boolean|null>}
     */
    async getMuteStatus(address, signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('mtd/'+address);
        if(false === signed) res = await this.peer.protocol.instance.get('mtd/'+address);
        if(null !== res) return res;
        return false;
    }

    /**
     *
     * @param signed
     * @returns {Promise<boolean>}
     */
    async getAutoAddWritersStatus(signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('auto_add_writers');
        if(false === signed) res = await this.peer.protocol.instance.get('auto_add_writers');
        if(null !== res) {
            return res === 'on';
        }
        return false;
    }

    /**
     *
     * @param signed
     * @returns {Promise<boolean>}
     */
    async getChatStatus(signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('chat_status');
        if(false === signed) res = await this.peer.protocol.instance.get('chat_status');
        if(null !== res) {
            return res === 'on';
        }
        return false;
    }

    /**
     *
     * @param nick
     * @param signed
     * @returns {Promise<null>}
     */
    async getAddressFromNick(nick, signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('kcin/'+nick);
        if(false === signed) res = await this.peer.protocol.instance.get('kcin/'+nick);
        return res;
    }

    /**
     *
     * @param address
     * @param signed
     * @returns {Promise<null>}
     */
    async getNick(address, signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('nick/'+address);
        if(false === signed) res = await this.peer.protocol.instance.get('nick/'+address);
        return res;
    }

    /**
     *
     * @param signed
     * @returns {Promise<number>}
     */
    async getPinnedMessageLength(signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('pnl');
        if(false === signed) res = await this.peer.protocol.instance.get('pnl');
        res = res !== null ? res : 0;
        return res;
    }

    /**
     *
     * @param index
     * @param signed
     * @returns {Promise<null>}
     */
    async getPinnedMessage(index, signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('pni/'+parseInt(index));
        if(false === signed) res = await this.peer.protocol.instance.get('pni/'+parseInt(index));
        if(null === res) {
            return null
        }
        return await this.getMessage(res.msg, signed);
    }

    /**
     *
     * @param signed
     * @returns {Promise<number>}
     */
    async getDeletedMessageLength(signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('delml');
        if(false === signed) res = await this.peer.protocol.instance.get('delml');
        res = res !== null ? res : 0;
        return res;
    }

    /**
     *
     * @param index
     * @param signed
     * @returns {Promise<null>}
     */
    async getDeletedMessage(index, signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('delm/'+parseInt(index));
        if(false === signed) res = await this.peer.protocol.instance.get('delm/'+parseInt(index));
        if(null === res) {
            return null
        }
        return await this.getMessage(res, signed);
    }

    /**
     *
     * @param signed
     * @returns {Promise<number>}
     */
    async getMessageLength(signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('msgl');
        if(false === signed) res = await this.peer.protocol.instance.get('msgl');
        res = res !== null ? res : 0;
        return res;
    }

    /**
     *
     * @param index
     * @param signed
     * @returns {Promise<null>}
     */
    async getMessage(index, signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('msg/'+parseInt(index));
        if(false === signed) res = await this.peer.protocol.instance.get('msg/'+parseInt(index));
        return res;
    }

    /**
     *
     * @param address
     * @param signed
     * @returns {Promise<number>}
     */
    async getUserMessageLength(address, signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('umsgl/'+address);
        if(false === signed) res = await this.peer.protocol.instance.get('umsgl/'+address);
        res = res !== null ? res : 0;
        return res;
    }

    /**
     *
     * @param address
     * @param index
     * @param signed
     * @returns {Promise<null>}
     */
    async getUserMessage(address, index, signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('umsg/'+address+'/'+parseInt(index));
        if(false === signed) res = await this.peer.protocol.instance.get('umsg/'+address+'/'+parseInt(index));
        if(null === res) {
            return null
        }
        return await this.getMessage(res, signed);
    }

    /**
     *
     * @param signed
     * @returns {Promise<number>}
     */
    async getTxLength(signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('txl');
        if(false === signed) res = await this.peer.protocol.instance.get('txl');
        res = res !== null ? res : 0;
        return res;
    }

    /**
     *
     * @param index
     * @param signed
     * @returns {Promise<null>}
     */
    async getTx(index, signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('txi/'+parseInt(index));
        if(false === signed) res = await this.peer.protocol.instance.get('txi/'+parseInt(index));
        return res;
    }

    /**
     *
     * @param tx
     * @param signed
     * @returns {Promise<null>}
     */
    async getTxData(tx, signed = true){
        let index = null;
        if(true === signed) index = await this.peer.protocol.instance.getSigned('tx/'+tx);
        if(false === signed) index = await this.peer.protocol.instance.get('tx/'+tx);
        if(null !== index) {
            return await this.getTx(index, signed);
        }
        return null;
    }

    /**
     *
     * @param address
     * @param signed
     * @returns {Promise<number>}
     */
    async getUserTxLength(address, signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('utxl/'+address);
        if(false === signed) res = await this.peer.protocol.instance.get('utxl/'+address);
        res = res !== null ? res : 0;
        return res;
    }

    /**
     *
     * @param address
     * @param index
     * @param signed
     * @returns {Promise<null>}
     */
    async getUserTx(address, index, signed = true){
        let res = null;
        if(true === signed) res = await this.peer.protocol.instance.getSigned('utxi/'+address+'/'+parseInt(index));
        if(false === signed) res = await this.peer.protocol.instance.get('utxi/'+address+'/'+parseInt(index));
        if(null !== res){
            return await this.getTx(res, signed);
        }
        return res;
    }
}

export default ProtocolApi;
