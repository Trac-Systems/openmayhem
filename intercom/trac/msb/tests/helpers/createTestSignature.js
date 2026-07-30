import { WalletProvider } from 'trac-wallet';
import { config } from './config.js';

export async function createSignature(payloadBuffer) {
    const wallet = await new WalletProvider(config).generate({ derivationPath: config.derivationPath })
    const signature = wallet.sign(payloadBuffer);
    return { signature, wallet };
}

export default {
    createSignature
};
