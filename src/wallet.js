import PeerWallet from "trac-wallet"
import b4a from "b4a";

class Wallet extends PeerWallet {
    constructor() {
        super();
    }

    get publicKey() {
        const pk = super.publicKey;
        if (!pk) return null;
        return b4a.toString(pk, "hex");
    }

    get secretKey() {
        const sk = super.secretKey;
        if (!sk) return null;
        return b4a.toString(sk, "hex");
    }

    sign(message) {
        const msgBuf = b4a.isBuffer(message) ? message : b4a.from(String(message));
        const signatureBuf = super.sign(msgBuf);
        return b4a.toString(signatureBuf, "hex");
    }

    verify(signature, message, publicKey = this.publicKey) {
        const sigBuf = b4a.isBuffer(signature) ? signature : b4a.from(String(signature), "hex");
        const msgBuf = b4a.isBuffer(message) ? message : b4a.from(String(message));
        const pkBuf = b4a.isBuffer(publicKey) ? publicKey : b4a.from(String(publicKey), "hex");
        return PeerWallet.verify(sigBuf, msgBuf, pkBuf);
    }
    
}

export default Wallet;
