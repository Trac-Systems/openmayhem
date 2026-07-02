import b4a from "b4a";
import { createMessage } from 'trac-msb/src/utils/buffer.js';
import { blake3 } from '@tracsystems/blake3';
import { MSB_OPERATION_TYPE } from '../msbClient.js';

class TerminalHandlers {
    #peer

    constructor(peer) {
        this.#peer = peer
    }

    async #getAdminKey(){
        const admin = await this.#peer.base.view.get('admin');
        return admin?.value ?? null;
    }

    async #isAdmin(){
        const target = this.#peer.wallet.publicKey
        const admin = await this.#getAdminKey();
        return admin !== null && admin === target;
    }

    async #isMod(){
        const target = this.#peer.wallet.publicKey
        const mod = await this.#peer.base.view.get('mod/' + target);
        return mod !== null && mod.value === true;
    }

    async #requireAdmin(){
        if (await this.#isAdmin()) return;
        throw new Error('Only admin may perform this operation.');
    }

    async #requireAdminOrMod(){
        if (await this.#isAdmin()) return;
        if (await this.#isMod()) return;
        throw new Error('Only admin or mod may perform this operation.');
    }

    #getSubnetBootstrapHex(){
        const b = this.#peer.config.bootstrap;
        if (!b) return null;
        if (typeof b === 'string') return b.toLowerCase();
        if (b?.toString) return b.toString('hex').toLowerCase();
        return null;
    }

    #getLocalWriterKeyHex(){
        try {
            return this.#peer?.base?.local?.key?.toString('hex') ?? this.#peer?.writerLocalKey ?? null;
        } catch (_e) {
            return this.#peer?.writerLocalKey ?? null;
        }
    }

    async #requireBootstrapNodeForAdminSet(){
        const bootstrapHex = this.#getSubnetBootstrapHex();
        const writerKeyHex = this.#getLocalWriterKeyHex();
        if (!bootstrapHex || !writerKeyHex) throw new Error('Peer is not initialized.');
        if (bootstrapHex !== writerKeyHex.toLowerCase()) {
            throw new Error('Only the subnet bootstrap node may set the initial admin.');
        }
        const admin = await this.#getAdminKey();
        if (admin !== null) throw new Error('Admin is already set.');
    }

    async setWhitelistStatus(input){
        await this.#requireAdmin();
        const splitted = this.#peer.protocol.instance.parseArgs(input)
        const value = ''+splitted.user;
        const status = parseInt(splitted.status) === 1;
        const nonce = this.#peer.protocol.instance.generateNonce();
        const signature = { dispatch : {
                type : 'setWhitelistStatus',
                user: value,
                status : status,
                address : this.#peer.wallet.publicKey
            }};
        const hash = this.#peer.wallet.sign(JSON.stringify(signature) + nonce);
        await this.#peer.base.append({type: 'setWhitelistStatus', value: signature, hash : hash, nonce: nonce });
    }

    async enableTransactions(input){
        const splitted = this.#peer.protocol.instance.parseArgs(input)
        const value = parseInt(splitted.enabled) === 1;
        const nonce = this.#peer.protocol.instance.generateNonce();
        const signature = { dispatch : {
                type : 'enableTransactions',
                enabled: value,
                address : this.#peer.wallet.publicKey
            }};
        const hash = this.#peer.wallet.sign(JSON.stringify(signature) + nonce);
        await this.#peer.base.append({type: 'enableTransactions', value: signature, hash : hash, nonce: nonce });
    }

    async enableWhitelist(input){
        await this.#requireAdmin();
        const splitted = this.#peer.protocol.instance.parseArgs(input)
        const value = parseInt(splitted.enabled) === 1;
        const nonce = this.#peer.protocol.instance.generateNonce();
        const signature = { dispatch : {
                type : 'enableWhitelist',
                enabled: value,
                address : this.#peer.wallet.publicKey
            }};
        const hash = this.#peer.wallet.sign(JSON.stringify(signature) + nonce);
        await this.#peer.base.append({type: 'enableWhitelist', value: signature, hash : hash, nonce: nonce });
    }

    async unpinMessage(input){
        await this.#requireAdminOrMod();
        const splitted = this.#peer.protocol.instance.parseArgs(input)
        const value = parseInt(splitted.pin_id);
        const nonce = this.#peer.protocol.instance.generateNonce();
        const signature = { dispatch : {
                type : 'unpinMessage',
                id: value,
                address : this.#peer.wallet.publicKey
            }};
        const hash = this.#peer.wallet.sign(JSON.stringify(signature) + nonce);
        await this.#peer.base.append({type: 'unpinMessage', value: signature, hash : hash, nonce: nonce });
    }

    async pinMessage(input){
        await this.#requireAdminOrMod();
        const splitted = this.#peer.protocol.instance.parseArgs(input)
        const value = parseInt(splitted.id);
        const pinned = parseInt(splitted.pin) === 1;
        const nonce = this.#peer.protocol.instance.generateNonce();
        const signature = { dispatch : {
                type : 'pinMessage',
                id: value,
                pinned : pinned,
                address : this.#peer.wallet.publicKey
            }};
        const hash = this.#peer.wallet.sign(JSON.stringify(signature) + nonce);
        await this.#peer.base.append({type: 'pinMessage', value: signature, hash : hash, nonce: nonce });
    }

    async deleteMessage(input){
        await this.#requireAdminOrMod();
        const splitted = this.#peer.protocol.instance.parseArgs(input)
        const value = parseInt(splitted.id);
        const nonce = this.#peer.protocol.instance.generateNonce();
        const signature = { dispatch : {
                type : 'deleteMessage',
                id: value,
                address : this.#peer.wallet.publicKey
            }};
        const hash = this.#peer.wallet.sign(JSON.stringify(signature) + nonce);
        await this.#peer.base.append({type: 'deleteMessage', value: signature, hash : hash, nonce: nonce });
    }

    async updateAdmin(input){
        await this.#requireAdmin();
        const splitted = this.#peer.protocol.instance.parseArgs(input)
        const value = ''+splitted.address === 'null' ? null : ''+splitted.address;
        const nonce = this.#peer.protocol.instance.generateNonce();
        const signature = { dispatch : {
                type : 'updateAdmin',
                admin: value,
                address : this.#peer.wallet.publicKey
            }};
        const hash = this.#peer.wallet.sign(JSON.stringify(signature) + nonce);
        await this.#peer.base.append({type: 'updateAdmin', value: signature, hash : hash, nonce: nonce });
    }

    async setMod(input){
        await this.#requireAdmin();
        const splitted = this.#peer.protocol.instance.parseArgs(input)
        const value = ''+splitted.user;
        const mod = parseInt(splitted.mod) === 1;
        const nonce = this.#peer.protocol.instance.generateNonce();
        const signature = { dispatch : {
                type : 'setMod',
                user: value,
                mod : mod,
                address : this.#peer.wallet.publicKey
            }};
        const hash = this.#peer.wallet.sign(JSON.stringify(signature) + nonce);
        await this.#peer.base.append({type: 'setMod', value: signature, hash : hash, nonce: nonce });
    }

    async muteStatus(input){
        await this.#requireAdminOrMod();
        const splitted = this.#peer.protocol.instance.parseArgs(input)
        const value = ''+splitted.user;
        const muted = parseInt(splitted.muted) === 1;
        const nonce = this.#peer.protocol.instance.generateNonce();
        const signature = { dispatch : {
                type : 'muteStatus',
                user: value,
                muted : muted,
                address : this.#peer.wallet.publicKey
            }};
        const hash = this.#peer.wallet.sign(JSON.stringify(signature) + nonce);
        await this.#peer.base.append({type: 'muteStatus', value: signature, hash : hash, nonce: nonce });
    }

    async setNick(input){
        const splitted = this.#peer.protocol.instance.parseArgs(input)
        const value = ''+splitted.nick;
        let user = null;
        if(splitted.user !== undefined){
            user = ''+splitted.user;
        }
        if (user !== null && user !== this.#peer.wallet.publicKey) {
            await this.#requireAdminOrMod();
        }
        const nonce = this.#peer.protocol.instance.generateNonce();
        const signature = { dispatch : {
                type : 'setNick',
                nick: value,
                address : null === user ? this.#peer.wallet.publicKey : user,
                initiator: this.#peer.wallet.publicKey
            }};
        const hash = this.#peer.wallet.sign(JSON.stringify(signature) + nonce);
        await this.#peer.base.append({type: 'setNick', value: signature, hash : hash, nonce: nonce });
    }

    async postMessage(input){
        const splitted = this.#peer.protocol.instance.parseArgs(input)
        if(typeof splitted.message === "boolean" || splitted.message === undefined) throw new Error('Empty message not allowed');
        const chat_status = await this.#peer.base.view.get('chat_status');
        if (chat_status === null || chat_status.value !== 'on') throw new Error('Chat is disabled.');
        const reply_to = splitted.reply_to !== undefined ? parseInt(splitted.reply_to) : null;
        const value = '' + splitted.message;
        const nonce = this.#peer.protocol.instance.generateNonce();
        const signature = { dispatch : {
                type : 'msg',
                msg: value,
                address : this.#peer.wallet.publicKey,
                attachments : [],
                deleted_by : null,
                reply_to : reply_to,
                pinned : false,
                pin_id : null
            }};
        const hash = this.#peer.wallet.sign(JSON.stringify(signature) + nonce);
        await this.#peer.base.append({type: 'msg', value: signature, hash : hash, nonce: nonce });
    }

    async setChatStatus(input){
        await this.#requireAdmin();
        const splitted = this.#peer.protocol.instance.parseArgs(input)
        const value = parseInt(splitted.enabled) === 1 ? 'on' : 'off';
        const nonce = this.#peer.protocol.instance.generateNonce();
        if(value !== 'on' && value !== 'off') throw new Error('setChatStatus: use on and off values.');
        const msg = { type: 'setChatStatus', key: value }
        const signature = {
            msg: msg
        };
        const hash = this.#peer.wallet.sign(JSON.stringify(msg) + nonce);
        await this.#peer.base.append({type: 'setChatStatus', key: value, value: signature, hash : hash, nonce: nonce });
    }

    async setAutoAddWriters(input){
        await this.#requireAdmin();
        const splitted = this.#peer.protocol.instance.parseArgs(input)
        const value = parseInt(splitted.enabled) === 1 ? 'on' : 'off';
        const nonce = this.#peer.protocol.instance.generateNonce();
        if(value !== 'on' && value !== 'off') throw new Error('setAutoAddWriters: use on and off values.');
        const msg = { type: 'setAutoAddWriters', key: value }
        const signature = {
            msg: msg
        };
        const hash = this.#peer.wallet.sign(JSON.stringify(msg) + nonce);
        await this.#peer.base.append({type: 'setAutoAddWriters', key: value, value: signature, hash : hash, nonce: nonce });
    }

    async addAdmin(input){
        await this.#requireBootstrapNodeForAdminSet();
        const splitted = this.#peer.protocol.instance.parseArgs(input)
        const publicKey = (splitted.address != null ? String(splitted.address) : '').trim().toLowerCase();
        if (!/^[0-9a-f]{64}$/.test(publicKey)) {
            throw new Error(
                'Invalid --address. Expected a 32-byte hex public key (64 hex chars). Example: /add_admin --address "<peer-publicKey-hex>"'
            );
        }
        await this.#addAdminKey(publicKey);
    }

    async #addAdminKey(publicKeyHex){
        await this.#requireBootstrapNodeForAdminSet();
        const publicKey = (publicKeyHex != null ? String(publicKeyHex) : '').trim().toLowerCase();
        if (!/^[0-9a-f]{64}$/.test(publicKey)) {
            throw new Error('Invalid public key. Expected 32-byte hex (64 chars).');
        }
        if(this.#peer.base.writable === false){
            throw new Error('Peer is not writable.');
        }
        await this.#peer.base.append({ type: 'addAdmin', key: publicKey });
    }

    async #appendAddWriter(keyHex, isIndexer){
        await this.#requireAdmin();
        const wk = (keyHex != null ? String(keyHex) : '').trim().toLowerCase();
        if (!/^[0-9a-f]{64}$/.test(wk)) throw new Error('Invalid --key. Expected 32-byte hex (64 chars).');
        const nonce = this.#peer.protocol.instance.generateNonce();
        const msg = { type: isIndexer ? 'addIndexer' : 'addWriter', key: wk };
        const signature = { msg: msg };
        const hash = this.#peer.wallet.sign(JSON.stringify(msg) + nonce);
        if(this.#peer.base.writable === false){
            throw new Error('Peer is not writable.');
        }
        await this.#peer.base.append({
            op : 'append_writer',
            type: isIndexer ? 'addIndexer' : 'addWriter',
            key: wk,
            value: signature,
            hash: hash,
            nonce: nonce
        });
    }

    async #addWriterKey(keyHex){
        return this.#appendAddWriter(keyHex, false);
    }

    async #addIndexerKey(keyHex){
        return this.#appendAddWriter(keyHex, true);
    }

    async addWriter(input){
        await this.#requireAdmin();
        const parsed = this.#peer.protocol.instance.parseArgs(input);
        return this.#addWriterKey(parsed.key);
    }

    async addIndexer(input){
        await this.#requireAdmin();
        const parsed = this.#peer.protocol.instance.parseArgs(input);
        return this.#addIndexerKey(parsed.key);
    }

    async #appendRemoveWriter(keyHex){
        await this.#requireAdmin();
        const wk = (keyHex != null ? String(keyHex) : '').trim().toLowerCase();
        if (!/^[0-9a-f]{64}$/.test(wk)) throw new Error('Invalid --key. Expected 32-byte hex (64 chars).');
        const nonce = this.#peer.protocol.instance.generateNonce();
        const msg = { type: 'removeWriter', key: wk };
        const signature = { msg: msg };
        const hash = this.#peer.wallet.sign(JSON.stringify(msg) + nonce);
        if(this.#peer.base.writable === false){
            throw new Error('Peer is not writable.');
        }
        await this.#peer.base.append({ op : 'remove_writer', type: 'removeWriter', key: wk, value: signature, hash: hash, nonce : nonce });
    }

    async #removeWriterKey(keyHex){
        return this.#appendRemoveWriter(keyHex);
    }

    async #removeIndexerKey(keyHex){
        // Current apply operation is `removeWriter` and it removes either writer or indexer.
        // This keeps the exact on-chain / on-log operation type unchanged.
        return this.#appendRemoveWriter(keyHex);
    }

    async removeWriter(input){
        await this.#requireAdmin();
        const parsed = this.#peer.protocol.instance.parseArgs(input);
        return this.#removeWriterKey(parsed.key);
    }

    async removeIndexer(input){
        await this.#requireAdmin();
        const parsed = this.#peer.protocol.instance.parseArgs(input);
        return this.#removeIndexerKey(parsed.key);
    }

    async verifyDag(_input){
        if (this.#peer.verifyDag) {
            return this.#peer.verifyDag();
        }
        try {
            console.log('--- Stats ---');
            const dagView = await this.#peer.base.view.core.treeHash();
            const lengthdagView = this.#peer.base.view.core.length;
            const dagSystem = await this.#peer.base.system.core.treeHash();
            const lengthdagSystem = this.#peer.base.system.core.length;
            console.log('wallet.address:', this.#peer.wallet !== null ? this.#peer.wallet.publicKey : 'unset');
            console.log('hypermall.writerKey:', this.#peer.writerLocalKey);
            const admin = await this.#peer.base.view.get('admin')
            console.log(`admin: ${admin !== null ? admin.value : 'unset'}`);
            console.log(`isIndexer: ${this.#peer.base.isIndexer}`);
            console.log(`isWriter: ${this.#peer.base.writable}`);
            console.log('swarm.connections.size:', this.#peer.swarm.connections.size);
            console.log('base.view.core.signedLength:', this.#peer.base.view.core.signedLength);
            console.log("base.signedLength", this.#peer.base.signedLength);
            console.log("base.indexedLength", this.#peer.base.indexedLength);
            console.log("base.linearizer.indexers.length", this.#peer.base.linearizer.indexers.length);
            console.log(`base.key: ${this.#peer.base.key.toString('hex')}`);
            console.log('discoveryKey:', b4a.toString(this.#peer.base.discoveryKey, 'hex'));
            console.log(`VIEW Dag: ${dagView.toString('hex')} (length: ${lengthdagView})`);
            console.log(`SYSTEM Dag: ${dagSystem.toString('hex')} (length: ${lengthdagSystem})`);
            const wl = await this.#peer.base.view.get('wrl');
            console.log('Total Registered Writers:', wl !== null ? wl.value : 0);
        } catch (error) {
            console.error('Error during DAG monitoring:', error.message);
        }
    }

    async tx(input){
        if(this.#peer.base?.writable === false) throw new Error('Peer is not writable.');
        const splitted = this.#peer.protocol.instance.parseArgs(input);
        let res = false;
        if(splitted.command === undefined){
            res = new Error('Missing option. Please use the --command flag.');
        }
        let sim = false;
        try{
            if(splitted.sim !== undefined && parseInt(splitted.sim) === 1){
                sim = true;
            }
            res = await this.#peer.protocol.instance.tx(splitted, sim);
        } catch(e){ console.log(e) }
        if(res !== false){
            const err = this.#peer.protocol.instance.getError(res);
            if(null !== err){
                console.log(err.message);
            } else if(res && typeof res === 'object') {
                if(res.txo && res.txo.tx) {
                    console.log('MSB TX broadcasted:', res.txo.tx);
                } else if(res.bdo && res.bdo.tx) {
                    console.log('MSB BOOTSTRAP_DEPLOYMENT broadcasted:', res.bdo.tx);
                }
            }
        }
        return res;
    }

    async deploySubnet(input){
        if(this.#peer.wallet.publicKey === null || this.#peer.wallet.secretKey === null) throw new Error('Wallet is not initialized.');

        const txvHex = await this.#peer.msbClient.getTxvHex();
        const nonceHex = this.#peer.protocol.instance.generateNonce();
        const subnetBootstrapHex = (b4a.isBuffer(this.#peer.config.bootstrap) ? this.#peer.config.bootstrap.toString('hex') : (''+this.#peer.config.bootstrap)).toLowerCase();
        const channelHex = b4a.isBuffer(this.#peer.config.channel) ? this.#peer.config.channel.toString('hex') : null;
        if(channelHex === null) throw new Error('Peer channel is not initialized.');

        const address = this.#peer.msbClient.pubKeyHexToAddress(this.#peer.wallet.publicKey);
        if(address === null) throw new Error('Failed to create MSB address from public key.');

        const msg = createMessage(
            this.#peer.msbClient.networkId,
            b4a.from(txvHex, 'hex'),
            b4a.from(subnetBootstrapHex, 'hex'),
            b4a.from(channelHex, 'hex'),
            b4a.from(nonceHex, 'hex'),
            MSB_OPERATION_TYPE.BOOTSTRAP_DEPLOYMENT
        );
        const txBuf = await blake3(msg);
        const txHex = b4a.toString(txBuf, 'hex');
        const signatureHex = this.#peer.wallet.sign(txBuf);

        const payload = {
            type: MSB_OPERATION_TYPE.BOOTSTRAP_DEPLOYMENT,
            address,
            bdo: {
                tx: txHex,
                txv: txvHex,
                bs: subnetBootstrapHex,
                ic: channelHex,
                in: nonceHex,
                is: signatureHex
            }
        };

        const ok = await this.#peer.msbClient.broadcastBootstrapDeployment(payload);
        if(ok !== true) throw new Error('Subnet deployment broadcast failed.');
        console.log('Subnet bootstrap:', subnetBootstrapHex);
        console.log('Subnet channel (hex):', channelHex);
        console.log('Subnet deployment tx:', payload.bdo.tx);
        return payload;
    }

    getKeys(){
        console.log("Address: ", this.#peer.wallet.address);
        console.log("Public Key: ", this.#peer.wallet.publicKey);
        console.log("Secret Key: ", this.#peer.wallet.secretKey);
    }

    async exit({ rl } = {}){
        console.log('Exiting...');
        if(rl) rl.close();
        await this.#peer.close();
        typeof process !== "undefined" ? process.exit(0) : Pear.exit(0);
        return { exit: true };
    }
}

export { TerminalHandlers };
