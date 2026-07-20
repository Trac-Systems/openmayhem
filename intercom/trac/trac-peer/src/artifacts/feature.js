class Feature {
    constructor(peer, config) {
        this.peer = peer;
        this.key = '';
        this.config = config;
    }

    async getSigned(key){
        const view_session = this.peer.base.view.checkout(this.peer.base.view.core.signedLength);
        const result = await view_session.get(key);
        await view_session.close();
        if(result === null) return null;
        return result.value;
    }

    async get(key){
        const result = await this.peer.base.view.get(key);
        if(result === null) return null;
        return result.value;
    }

    async append(key, value){
        if(this.peer.base.writable){
            const nonce = this.peer.protocol.instance.generateNonce();
            const hash = this.peer.wallet.sign(JSON.stringify(value) + nonce);
            await this.peer.base.append({ type: 'feature', key: this.key + '_' + key, value : {
                    dispatch : {
                        type : this.key + '_feature',
                        key : key,
                        hash : hash,
                        value : value,
                        nonce : nonce,
                        address : this.peer.wallet.publicKey
                    }
                }});
        } else {
            console.log('Peer running features not writable.');
        }
    }

    async start(options = {}) {
        throw Error('Not implemented: start(options = {})');
    }

    async stop(options = {}) {
        throw Error('Not implemented: stop(options = {})');
    }

    async sleep(ms) {
        return new Promise(resolve => setTimeout(resolve, ms));
    }
}

export default Feature;