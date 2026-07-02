import PeerWallet from 'trac-wallet';
import b4a from 'b4a';

class NetworkWalletFactory {
    static provide(options = {}) {
        const {
            enableWallet,
            wallet,
            keyPair,
            networkPrefix
        } = options;

        if (enableWallet) {
            if (!wallet) {
                throw new Error('NetworkingWalletFactory: wallet instance is required when wallet is enabled');
            }
            return wallet;
        }

        if (!keyPair) {
            throw new Error('NetworkingWalletFactory: keyPair must be provided when wallet is disabled');
        }

        return new EphemeralWallet(keyPair, networkPrefix);
    }
}

// TODO: Once Wallet class in trac-wallet exposes a constructor/factory that accepts an existing keyPair
// (e.g. Wallet.fromKeyPair({ publicKey, secretKey }, networkPrefix)), replace EphemeralWallet
// with a thin wrapper around that functionality instead of duplicating signing/verification logic.
export class EphemeralWallet {
    #publicKey;
    #secretKey;
    #address;

    constructor(keyPair, networkPrefix) {
        
        if (!keyPair?.publicKey || !keyPair?.secretKey) {
            throw new Error('NetworkIdentityProvider: keyPair with publicKey and secretKey is required');
        }
        this.#assertBuffer(keyPair.publicKey);
        this.#assertBuffer(keyPair.secretKey);

        const address = PeerWallet.encodeBech32m(networkPrefix, keyPair.publicKey);
        if (!address) {
            throw new Error('NetworkIdentityProvider: failed to derive address from networking key pair');
        }

        this.#publicKey = keyPair.publicKey;
        this.#secretKey = keyPair.secretKey;
        this.#address = address;
    }

    get publicKey() {
        return this.#publicKey;
    }

    get address() {
        return this.#address;
    }

    sign(message) {
        return PeerWallet.sign(message, this.#secretKey);
    }

    verify(signature, message, publicKey = this.#publicKey) {
        return PeerWallet.verify(signature, message, publicKey);
    }

    #assertBuffer(value) {
        if (!b4a.isBuffer(value)) {
            throw new Error(`NetworkIdentityProvider: value must be a Buffer`);
        }
    }
}

export default NetworkWalletFactory;
