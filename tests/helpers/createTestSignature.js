import PeerWallet from 'trac-wallet';

export async function createSignature(payloadBuffer) {
    const wallet = new PeerWallet();
    await wallet.generateKeyPair();
    const signature = wallet.sign(payloadBuffer);
    return { signature, wallet };
}

export default {
    createSignature
};
