/** @typedef {import('pear-interface')} */ /* global Pear */
import Autobase from 'autobase';
import Hyperswarm from 'hyperswarm';
import ReadyResource from 'ready-resource';
import b4a from 'b4a';
import Hyperbee from 'hyperbee';
import Corestore from 'corestore';
import w from 'protomux-wakeup';
const wakeup = new w();
import Protomux from 'protomux'
import c from 'compact-encoding'
import { MsbClient } from './msbClient.js';
import { handlerFor } from './operations/index.js';
import TransactionPool from './transaction/transactionPool.js';
import { TransactionObserver } from './tasks/transactionObserver.js';
import { Updater } from './tasks/updater.js';
export {default as Protocol} from "./artifacts/protocol.js";
export {default as Contract} from "./artifacts/contract.js";
export {default as Feature} from "./artifacts/feature.js";
export {default as Wallet} from "./wallet.js";
export { ENV, createConfig } from './config/env.js';
export { Config } from './config/config.js';

export class Peer extends ReadyResource {
    constructor(options) {
        super();

        const { config, msb, wallet, protocol, contract, features = [], readlineInstance = null } = options;

        this.config = config;

        this.keyPair = null;
        this.store = new Corestore(this.config.fullStoresDirectory);
        this.msbClient = new MsbClient(msb);
        this.swarm = null;
        this.base = null;
        this.key = null;
        this.txPool = new TransactionPool(this.config);
        this.txObserverTask = null;
        this.updaterTask = null;
        this.writerLocalKey = null;

        this.wallet = wallet        
        this.protocol = { Class: protocol, instance: null }
        this.contract = { Class: contract, instance: null }
        this.features = features || [];
        
        // In bare runtime, Buffer#fill(undefined) throws; default to 0 when channel not provided.
        this.bee = null;
        this.connectedNodes = 1;
        this.connectedPeers = new Set();
        this.readlineInstance = readlineInstance || null;
    }

    async _open() {
        await this.msbClient.ready()
        this._boot();
        await this.base.ready();

        if (this.config.enableBackgroundTasks) this.txObserver();

        if (this.config.bootstrap === null) {
            this.config.bootstrap = this.base.key;
        }
        await this.wallet.initKeyPair(this.config.keyPairPath, this.readlineInstance);
        this.writerLocalKey = b4a.toString(this.base.local.key, 'hex');

        await this.initContract();

        if (this.config.replicate) await this._replicate();
        if (this.config.enableUpdater) {
            this.updaterTask = new Updater({ base: this.base }, this.config)
            this.updaterTask.start()
        }
    }

    async _boot() {
        this.base = new Autobase(this.store, this.config.bootstrap, {
            ackInterval : 1000,
            valueEncoding: 'json',
            open: store => {
                this.bee = new Hyperbee(store.get('view'), {
                    extension: false,
                    keyEncoding: 'utf-8',
                    valueEncoding: 'json'
                })
                return this.bee;
            },
            apply: async (nodes, view, base) => {
                const batch = view.batch()
                const context = {
                    wallet: this.wallet,
                    protocolInstance: this.protocol.instance,
                    contractInstance: this.contract.instance,
                    msbClient: this.msbClient,
                    config: this.config
                }

                for (const node of nodes) {
                    const op = node.value
                    const handler = handlerFor(node, context)
                    if (handler) {
                        await handler.handle(op, batch, base, node)
                    }
                }

                await batch.flush();
                await batch.close();
            }
        })
        this.base.on('warning', (e) => console.log(e))
    }

    async sendTx(msg){
        if(msg?.type === 'number') {
            try { await this.msbClient.broadcastTransaction(msg); } catch(_e) { }
        }
    }

    async initContract(){
        this.protocol.instance = new this.protocol.Class(this, this.base, this.config);
        await this.protocol.instance.extendApi();
        this.contract.instance = new this.contract.Class(this.protocol.instance, this.config);
    }

    async close() {
        if (this.txObserverTask) this.txObserverTask.stop()
        if (this.updaterTask) this.updaterTask.stop()
        if (this.swarm) {
            await this.swarm.destroy();
        }
        await this.base.close();
    }

    async txObserver() {
        if (!this.txObserverTask) {
            this.txObserverTask = new TransactionObserver(
                { base: this.base, msbClient: this.msbClient, txPool: this.txPool },
                this.config
            );
        }
        this.txObserverTask.start()
    }

    async sleep(ms) {
        return new Promise(resolve => setTimeout(resolve, ms));
    }

    async _replicate() {
        if (!this.swarm) {

            const _this = this;

            const keyPair = {
                publicKey: b4a.from(this.wallet.publicKey, 'hex'),
                secretKey: b4a.from(this.wallet.secretKey, 'hex')
            };

            this.swarm = new Hyperswarm({ keyPair, bootstrap: this.config.dhtBootstrap });
            console.log('');
            console.log('######################################################################################');
            console.log('# Peer Address:    ', this.wallet.publicKey, '#');
            this.writerLocalKey = b4a.toString(this.base.local.key, 'hex');
            console.log('# Peer Writer:     ', this.writerLocalKey, '#');
            console.log('######################################################################################');
            console.log('');
            console.log(`isIndexer: ${this.base.isIndexer}`);
            console.log(`isWriter: ${this.base.writable}`);
            console.log('');

            const subnetBootstrapHex = b4a.isBuffer(this.config.bootstrap)
                ? b4a.toString(this.config.bootstrap, 'hex')
                : String(this.config.bootstrap ?? '').toLowerCase();

            this.swarm.on('connection', async (connection, peerInfo) => {
                const mux = Protomux.from(connection)
                connection.userData = mux
                const message_channel = mux.createChannel({
                    protocol: b4a.toString(this.config.channel, 'utf8'),
                    onopen() {},
                    onclose() {}
                })
                message_channel.open()
                const message = message_channel.addMessage({
                    encoding: c.json,
                    async onmessage(msg) {
                        try{
                            if(true === _this.base.writable && msg.inviteMyKey !== undefined &&
                                typeof msg.bootstrap === 'string' &&
                                msg.bootstrap.toLowerCase() === subnetBootstrapHex &&
                                typeof msg.to === 'string' &&
                                msg.to.toLowerCase() === String(_this.wallet?.publicKey ?? '').toLowerCase()){
                                const auto_add_writers = await _this.base.view.get('auto_add_writers');
                                if(auto_add_writers !== null && auto_add_writers.value === 'on') {
                                    await _this.base.append({
                                        type : 'autoAddWriter',
                                        key : msg.inviteMyKey
                                    });
                                }
                            }
                        }catch(e){}
                    }});

                if(false === _this.base.writable){
                    // Only request writer access when this subnet has auto_add_writers enabled (after replication).
                    const tryInvite = async () => {
                        const shouldInvite = async () => {
                            const auto_add_writers = await _this.base.view.get('auto_add_writers');
                            return auto_add_writers !== null && auto_add_writers.value === 'on';
                        };

                        if (await shouldInvite()) {
                            message.send({
                                inviteMyKey : _this.writerLocalKey,
                                bootstrap : subnetBootstrapHex,
                                to : b4a.toString(connection.remotePublicKey, 'hex')
                            });
                            return;
                        }

                        // Wait for replication to catch up (bounded), then re-check.
                        const deadline = Date.now() + 15_000;
                        while (Date.now() < deadline) {
                            await new Promise((resolve) => _this.base.view.core.once('append', resolve));
                            if (await shouldInvite()) {
                                message.send({
                                    inviteMyKey : _this.writerLocalKey,
                                    bootstrap : subnetBootstrapHex,
                                    to : b4a.toString(connection.remotePublicKey, 'hex')
                                });
                                return;
                            }
                        }
                    };

                    tryInvite().catch(() => {});
                }

                const remotePublicKey = b4a.toString(connection.remotePublicKey, 'hex');

                this.connectedPeers.add(remotePublicKey);
                const stream = this.store.replicate(connection);
                stream.on('error', (error) => { });
                wakeup.addStream(stream);
                this.connectedNodes++;

                connection.on('close', () => {
                    try{ message_channel.close() }catch(e){}
                    this.connectedNodes--;
                    this.connectedPeers.delete(remotePublicKey);
                });

                connection.on('error', (error) => { });
            });

            this.swarm.join(this.config.channel, { server: true, client: true });
            await this.swarm.flush();
        }
    }
}

export default Peer;
