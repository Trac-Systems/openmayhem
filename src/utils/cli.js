import b4a from 'b4a';
import { safeDecodeApplyOperation } from "./protobuf/operationHelpers.js";
import { bigIntToDecimalString, bufferToBigInt } from "./amountSerialization.js";

export async function verifyDag(state, network, wallet, writerKey) {
    try {
        console.log('---------- node & network stats ----------');
        let dagView, lengthdagView, dagSystem, lengthdagSystem;

        if (state?.base?.view?.core && state?.base?.system?.core) {
            try {
                dagView = await state.base.view.core.treeHash();
                lengthdagView = state.base.view.core.length;
                dagSystem = await state.base.system.core.treeHash();
                lengthdagSystem = state.base.system.core.length;
            } catch (e) {
                console.log('Error getting DAG info:', e.message);
            }
        }

        console.log('wallet.publicKey:', wallet?.publicKey?.toString('hex') ?? 'unset');
        console.log('wallet.address:', (wallet && wallet.address) ? wallet.address : 'unset');
        console.log('msb.writerKey:', writerKey ? writerKey.toString('hex') : 'unset');
        console.log('swarm.connections.size:', network?.swarm?.connections?.size || 0);
        if (state?.base?.view?.core) {
            console.log('base.view.core.signedLength:', state.base.view.core.signedLength ?? 'unset');
            console.log('base.view.core.length:', state.base.view.core.length ?? 'unset');
        }

        if (state?.base) {
            console.log("base.signedLength", state.base.signedLength ?? 'unset');
            console.log("base.indexedLength", state.base.indexedLength ?? 'unset');
            console.log("base.linearizer.indexers.length", state.base.linearizer?.indexers?.length ?? 'unset');
            console.log(`base.key: ${state.base.key ? state.base.key.toString('hex') : 'unset'}`);
            console.log('discoveryKey:', state.base.discoveryKey ? b4a.toString(state.base.discoveryKey, 'hex') : 'unset');
        }
        console.log(`VIEW Dag: ${dagView ? dagView.toString('hex') : 'unset'} (length: ${lengthdagView || 0})`);
        console.log(`SYSTEM Dag: ${dagSystem ? dagSystem.toString('hex') : 'unset'} (length: ${lengthdagSystem || 0})`);
        const wl = await state.getWriterLength();
        console.log('Total Registered Writers:', wl !== null ? wl : 0);

        console.log("---------- flags ----------");
        try {
            console.log(`isIndexer: ${state?.isIndexer?.() ?? 'unset'}`);
            console.log(`isWriter: ${state?.isWritable?.() ?? 'unset'}`);
        } catch (e) {
            console.log('Error getting flags:', e.message);
        }

    } catch (error) {
        console.error('Error during DAG monitoring:', error.message);
    }
}

export function printHelp(isAdminMode = false) {
    if (isAdminMode) {
        console.log('🚨 WARNING: IF YOU ARE NOT AN ADMIN, DO NOT USE THE COMMANDS BELOW! YOU RISK LOSING YOUR FUNDS! 🚨');
        console.log('\nAdmin commands:');
        console.log('- /add_admin: Register admin entry with bootstrap key (initial setup), or use --recovery flag to recover admin role');
        console.log('- /balance_migration: Perform balance migration with the given initial balances CSV file');
        console.log('- /add_whitelist: Add all specified whitelist addresses. If initialization is enabled, no fee is required.');
        console.log('- /disable_initialization: Disable further balance initializations and whitelisting');
        console.log('- /add_indexer <address>: Change a role of the selected writer node to indexer role. Charges a fee.');
        console.log('- /remove_indexer <address>: Change a role of the selected indexer node to default role. Charges a fee.');
        console.log('- /ban_writer <address>: Demote a whitelisted writer to default role and remove it from the whitelist. Charges a fee.');
    }
    console.log('Available commands:');
    console.log('- /add_writer: Add yourself as a validator to this MSB once whitelisted. Requires a fee + 10x the fee as a stake in $TNK.');
    console.log('- /remove_writer: Remove yourself from this MSB. Requires a fee, and the stake will be refunded.');
    console.log('- /node_status <address>: Get network information about a node with the given address.');
    console.log('- /stats: Check system stats such as writing key, DAG, etc.');
    console.log('- /deployment <subnetwork_bootstrap> <channel>: Deploy a subnetwork with the given bootstrap. If channel is not provided, a random one will be generated. Requires a fee.');
    console.log('- /get_deployment <subnetwork_bootstrap>: Get information about a subnetwork deployment with the given bootstrap.');
    console.log('- /transfer <to_address> <amount>: Transfer the specified amount to the given address. Requires a fee.');
    console.log('- /get_tx_info <tx_hash>: Get information about a transaction with the given hash.');
    console.log('- /get_validator_addr <writing_key>: Get the validator address mapped to the given writing key.');
    console.log('- /get_balance <address> <confirmed>: Get the balance of the node with specified address (confirmed = true is default)');
    console.log('- /exit: Exit the program.');
    console.log('- /help: Display this help.');
}

export const printWalletInfo = (address, writingKey) => {
    console.log('');
    console.log('#####################################################################################');
    console.log('# MSB Address:   ', address, ' #');
    console.log('# MSB Writer:    ', writingKey.toString('hex'), '#');
    console.log('#####################################################################################');
}

export const printBalance = async (address, state, wallet_enabled) => {
    if (wallet_enabled && state) {
        const nodeEntry = await state.getNodeEntry(address);
        const balance = nodeEntry ? bigIntToDecimalString(bufferToBigInt(nodeEntry.balance)) : '0';
        console.log(`Balance: ${balance}`);
    }
}

export const get_confirmed_tx_info = async (state_instance, txHash) => {
    const payload = await state_instance.getSigned(txHash);
    if (!payload) {
        return null
    }

    const decoded = safeDecodeApplyOperation(payload);
    if (!decoded) {
        throw new Error(`Failed to decode payload for transaction hash: ${txHash}`);
    }

    return {
        payload,
        decoded
    }
}

export const get_unconfirmed_tx_info = async (state_instance, txHash) => {
    const payload = await state_instance.get(txHash);
    if (!payload) {
        return null
    }

    const decoded = safeDecodeApplyOperation(payload);
    if (!decoded) {
        throw new Error(`Failed to decode payload for transaction hash: ${txHash}`);
    }

    return {
        payload,
        decoded
    }
}
