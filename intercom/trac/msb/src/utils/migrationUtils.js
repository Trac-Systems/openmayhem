
import { isAddressValid } from '../core/state/utils/address.js';
import PeerWallet from 'trac-wallet';
import b4a from 'b4a';
import { ZERO_LICENSE } from '../core/state/utils/nodeEntry.js';

export async function validateAddressFromIncomingFile(stateInstance, config, address, adminEntry) {
    if (!isAddressValid(address, config.addressPrefix)) {
        throw new Error(`Invalid address format: '${address}'. Please ensure all addresses are valid.`);
    }

    const publicKey = PeerWallet.decodeBech32m(address);

    if (!publicKey || publicKey.length !== 32) {
        throw new Error(`Invalid public key: '${address}'. Please ensure all addresses are valid.`);
    }

    if (address === adminEntry.address) {
        throw new Error(`The admin address '${address}' cannot be included in the current operation.`);
    }

    const nodeEntry = await stateInstance.getNodeEntryUnsigned(address);

    if (nodeEntry && nodeEntry.isWhitelisted) {
        throw new Error(`Whitelisted node address '${address}' cannot be included in the current operation.`);
    }

    if (nodeEntry && !b4a.equals(nodeEntry.license, ZERO_LICENSE)) {
        throw new Error(`Address '${address}' has been banned/whitelisted in the past and cannot be included in the current operation.`);
    }
}

export default {
    validateAddressFromIncomingFile
};