import PeerWallet from 'trac-wallet';
import b4a from 'b4a';

export const mnemonicAdmin = "science edit ankle purity treat unable first express scatter depend also case nose regular rally area carbon wait power corn sibling metal crop farm";

export const writingKeyAdmin = b4a.from("0768953b234c79eccc6306fdcba2d7c1f0b05b9af6815a3502e96a83a8878ff7", 'hex');
export const writingKeyNonAdmin = b4a.from("1768953b234c79eccc6306fdcba2d7c1f0b05b9af6815a3502e96a83a8878ff8", 'hex');

export const bootstrapAdmin = writingKeyAdmin;

export const walletAdmin = new PeerWallet();
export const walletNonAdmin = new PeerWallet();
export const walletPeer = new PeerWallet();

export const adminEntry = {
    address: null,
    wk: null,
}

export const initAll = async () => {
    if (!walletAdmin.publicKey) await walletAdmin.generateKeyPair(mnemonicAdmin);
    if (!walletNonAdmin.publicKey) await walletNonAdmin.generateKeyPair(walletNonAdmin.generateMnemonic());
    if (!walletPeer.publicKey) await walletPeer.generateKeyPair(walletPeer.generateMnemonic());
    if (!adminEntry.address && !adminEntry.wk) {
        adminEntry.address = walletAdmin.address;
        adminEntry.wk = writingKeyAdmin;
    }
}

export default {
    mnemonicAdmin,
    writingKeyAdmin,
    writingKeyNonAdmin,
    bootstrapAdmin,
    walletAdmin,
    walletNonAdmin,
    walletPeer,
    adminEntry,
    initAll,
}