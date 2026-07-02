import sodium from 'sodium-native';
import {generateMnemonic, mnemonicToSeed} from 'bip39-mnemonic';
import b4a from 'b4a'
import PeerWallet from "trac-wallet"
import path from 'path';
import {MainSettlementBus} from '../../src/index.js'
import { createConfig, ENV } from '../../src/config/env.js'
import fileUtils from '../../src/utils/fileUtils.js'
import {EntryType} from '../../src/utils/constants.js';
import {sleep} from '../../src/utils/helpers.js'
import {formatIndexersEntry} from '../../src/utils/helpers.js';
import { applyStateMessageFactory } from "../../src/messages/state/applyStateMessageFactory.js";
import { safeEncodeApplyOperation } from "../../src/utils/protobuf/operationHelpers.js"
import { $TNK } from '../../src/core/state/utils/balance.js';
import { EventType } from '../../src/utils/constants.js';
import { Config } from '../../src/config/config.js';
let os, fsp;

/**
 * Ensures that the environment is ready for file system and OS operations.
 */

async function ensureEnvReady() {
    if (!os || !fsp) {
        if (typeof globalThis.Bare !== 'undefined') {
            const bareOS = await import('bare-os');
            os = bareOS.default || bareOS;
            const bareFS = await import('bare-fs');
            fsp = (bareFS.default || bareFS).promises;
        } else {
            const nodeOS = await import('os');
            os = nodeOS.default || nodeOS;
            const nodeFS = await import('fs/promises');
            fsp = nodeFS.default || nodeFS;
        }
    }
}

export function randomBytes(num) {
    const buf = b4a.allocUnsafe(num);
    sodium.randombytes_buf(buf);
    return buf;
}

async function randomKeypair() {
    const keypair = {};
    const mnemonic = generateMnemonic();
    const seed = await mnemonicToSeed(mnemonic);

    const publicKey = b4a.alloc(sodium.crypto_sign_PUBLICKEYBYTES);
    const secretKey = b4a.alloc(sodium.crypto_sign_SECRETKEYBYTES);

    const hash = b4a.alloc(sodium.crypto_hash_sha256_BYTES);
    sodium.crypto_hash_sha256(hash, b4a.from(seed));

    const seed32 = b4a.from(hash, 'hex');

    sodium.crypto_sign_seed_keypair(publicKey, secretKey, seed32);

    keypair.publicKey = publicKey;
    keypair.secretKey = secretKey;
    return keypair;
}

export const tick = () => new Promise(resolve => setImmediate(resolve));

export async function fundPeer(admin, toFund, amount) {
    const txValidity = await admin.msb.state.getIndexerSequenceState()
    const payload = await applyStateMessageFactory(admin.wallet, admin.config)
        .buildCompleteBalanceInitializationMessage(
            admin.wallet.address,
            toFund.wallet.address,
            amount,
            txValidity
        );

    await admin.msb.state.append(safeEncodeApplyOperation(payload));
    await tick()
    await admin.msb.state.base.forceFastForward() // required to update the balance on the peer, eliminates the possible racing condition
    await tick()
    return payload
}

export async function initMsbPeer(peerName, peerKeyPair, temporaryDirectory, options = {}) {
    const peer = await initDirectoryStructure(peerName, peerKeyPair, temporaryDirectory);
    peer.options = options
    peer.options.storesDirectory = peer.storesDirectory;
    peer.options.storeName = peer.storeName;
    peer.config = createConfig(ENV.DEVELOPMENT, peer.options)
    const msb = new MainSettlementBus(peer.config);

    peer.msb = msb;
    peer.wallet = msb.wallet;
    peer.name = peerName;

    return peer;
}

export async function setupMsbPeer(peerName, peerKeyPair, temporaryDirectory, options = {}) {
    const peer = await initMsbPeer(peerName, peerKeyPair, temporaryDirectory, options);
    await peer.msb.ready();
    return peer;
}

export async function initMsbAdmin(keyPair, temporaryDirectory, options = {}) {
    const peerName = 'admin';
    const admin = await initMsbPeer(peerName, keyPair, temporaryDirectory, { ...options, bootstrap: randomBytes(32).toString('hex') });

    await admin.msb.ready();
    admin.options.bootstrap = admin.msb.state.writingKey.toString('hex');
    admin.config = new Config(admin.options, admin.config)
    await admin.msb.close();

    admin.msb = new MainSettlementBus(admin.config);
    await admin.msb.ready();
    await admin.msb.state.append(null); // before initialization system.indexers is empty, we need to initialize first block to create system.indexers array
    return admin;
}

export async function setupMsbAdmin(keyPair, temporaryDirectory, options = {}) {
    const admin = await initMsbAdmin(keyPair, temporaryDirectory, options);
    const txValidity = await admin.msb.state.getIndexerSequenceState();
    const payload = await applyStateMessageFactory(admin.wallet, admin.config)
        .buildCompleteAddAdminMessage(admin.wallet.address, admin.msb.state.writingKey, txValidity);

    await admin.msb.state.append(safeEncodeApplyOperation(payload));
    await tick();
    return admin;
}

export async function setupNodeAsWriter(admin, writerCandidate) {
    try {
        await setupWhitelist(admin, [writerCandidate.wallet.address]); // ensure if is whitelisted

        const validity = await admin.msb.getIndexerSequenceState()
        const req = await applyStateMessageFactory(writerCandidate.wallet, admin.config)
            .buildPartialAddWriterMessage(
                writerCandidate.wallet.address,
                b4a.toString(writerCandidate.msb.state.writingKey, 'hex'),
                b4a.toString(validity, 'hex'),
                'json'
            );

        await waitWritable(admin, writerCandidate, async () => {
            const payload = await applyStateMessageFactory(admin.wallet, admin.config)
                .buildCompleteAddWriterMessage(
                    admin.wallet.address,
                    b4a.from(req.rao.tx, 'hex'),
                    b4a.from(req.rao.txv, 'hex'),
                    b4a.from(req.rao.iw, 'hex'),
                    b4a.from(req.rao.in, 'hex'),
                    b4a.from(req.rao.is, 'hex')
                );
            await admin.msb.state.append(safeEncodeApplyOperation(payload))
        })

        return writerCandidate;
    } catch (error) {
        throw new Error('Error setting up MSB writer: ', error.message || error);
    }
}

export async function promoteToWriter(admin, writerCandidate) {
    await setupWhitelist(admin, [writerCandidate.wallet.address]);
    await waitForNodeState(writerCandidate,
        writerCandidate.wallet.address,
        {
            wk: writerCandidate.msb.state.writingKey,
            isWhitelisted: true,
            isWriter: false,
            isIndexer: false,
        })
    const validity = await admin.msb.state.getIndexerSequenceState()
    const req = await applyStateMessageFactory(writerCandidate.wallet, writerCandidate.config)
        .buildPartialAddWriterMessage(
            writerCandidate.wallet.address,
            b4a.toString(writerCandidate.msb.state.writingKey, 'hex'),
            b4a.toString(validity, 'hex'),
            'json'
        );

    await waitWritable(writerCandidate, writerCandidate, async () => {
        const payload = await applyStateMessageFactory(admin.wallet, admin.config)
            .buildCompleteAddWriterMessage(
                req.address,
                b4a.from(req.rao.tx, 'hex'),
                b4a.from(req.rao.txv, 'hex'),
                b4a.from(req.rao.iw, 'hex'),
                b4a.from(req.rao.in, 'hex'),
                b4a.from(req.rao.is, 'hex')
            );
        await admin.msb.state.append(safeEncodeApplyOperation(payload))
    })

    return writerCandidate;
}

export async function setupMsbWriter(admin, peerName, peerKeyPair, temporaryDirectory, options = {}) {
    try {
        const writerCandidate = await setupMsbPeer(peerName, peerKeyPair, temporaryDirectory, options);
        await fundPeer(admin, writerCandidate, $TNK(10n)) // It is assumed that a writer will write and therefore, will need money to process stuff
        return await promoteToWriter(admin, writerCandidate)
    } catch (error) {
        throw new Error('Error setting up MSB writer: ', error.message || error);
    }
}

export async function setupMsbIndexer(indexerCandidate, admin) {
    try {
    const validity = await admin.msb.state.getIndexerSequenceState()
    const payload = await applyStateMessageFactory(admin.wallet, admin.config)
        .buildCompleteAddIndexerMessage(admin.wallet.address, indexerCandidate.wallet.address, validity);

        await admin.msb.state.append(safeEncodeApplyOperation(payload));
        await tick(); // wait for the request to be processed

        const isIndexer = async () => {
            const indexersEntry = await admin.msb.state.getIndexersEntry();
            if (!indexersEntry) {
                return false;
            }
            const formatted = formatIndexersEntry(indexersEntry, admin.config.addressLength);
            if (!formatted || !formatted.addresses) return false;
            return formatted.addresses.includes(indexerCandidate.wallet.address);
        }

        let counter;
        const limit = 10; // maximum number of attempts to verify the role
        for (counter = 0; counter < limit; counter++) {
            if (await isIndexer(indexerCandidate.wallet.address) && await indexerCandidate.msb.state.isIndexer()) {
                break;
            }
            await indexerCandidate.msb.state.append(null);
            await indexerCandidate.msb.state.base.forceFastForward();
            await indexerCandidate.msb.state.base.view.update();
            await indexerCandidate.msb.network.swarm.flush()
            await admin.msb.state.base.update()
            await sleep(1000); // wait for the peer to sync state
        }

        return indexerCandidate;
    } catch (error) {
        throw new Error('Error setting up MSB indexer: ', error.message || error);
    }
}

export async function setupWhitelist(admin, whitelistAddresses) {
    if (!admin || !admin.msb || !admin.wallet) {
        throw new Error('Admin is not properly initialized');
    }

    if (!Array.isArray(whitelistAddresses) || whitelistAddresses.length === 0) {
        throw new Error('Whitelist addresses must be a non-empty array');
    }

    const adminEntry = await admin.msb.state.get(EntryType.ADMIN);
    if (!adminEntry) {
        throw new Error('Admin is not initialized. Execute /add_admin command first.');
    }
    // set up mock whitelist
    const originalReadAddressesFromWhitelistFile = fileUtils.readAddressesFromWhitelistFile;
    fileUtils.readAddressesFromWhitelistFile = async () => whitelistAddresses;
    const validity = await admin.msb.state.getIndexerSequenceState()
    for (const address of whitelistAddresses) {
        const payload = await applyStateMessageFactory(admin.wallet, admin.config)
            .buildCompleteAppendWhitelistMessage(admin.wallet.address, address, validity);

        await admin.msb.state.append(safeEncodeApplyOperation(payload));
        await sleep(100)
    }

    fileUtils.readAddressesFromWhitelistFile = originalReadAddressesFromWhitelistFile;
}

export async function initTemporaryDirectory() {
    await ensureEnvReady();
    const tmpDir = os.tmpdir();
    const unique = `tempTestStore-${Date.now()}-${Math.random().toString(16).slice(2)}`;
    const temporaryDirectory = path.join(tmpDir, unique);
    await fsp.mkdir(temporaryDirectory, {recursive: true});
    console.log('temporary directory: ', temporaryDirectory);
    return temporaryDirectory;
}

export async function removeTemporaryDirectory(temporaryDirectory) {
    await ensureEnvReady();
    await fsp.rm(temporaryDirectory, {recursive: true, force: true})
}

export async function initDirectoryStructure(peerName, keyPair, temporaryDirectory) {
    try {
        await ensureEnvReady();
        const storesDirectory = temporaryDirectory + '/stores/';
        const storeName = peerName + '/';
        const corestoreDbDirectory = path.join(storesDirectory, storeName, 'db');
        await fsp.mkdir(corestoreDbDirectory, {recursive: true});

        const keypath = path.join(corestoreDbDirectory, 'keypair.json');
        if (!keyPair || !keyPair.publicKey || !keyPair.secretKey) {
            keyPair = await randomKeypair();
        }
        const wallet = new PeerWallet(keyPair)
        await wallet.ready
        await wallet.exportToFile(keypath)
        return {
            storesDirectory,
            storeName,
            corestoreDbDirectory,
            keypath,
        }
    } catch (error) {
        throw new Error('Error creating directory structure: ' + error)
    }
}

export const deployExternalBootstrap = async (writer, externalNode) => {
    const externalBootstrap = randomBytes(32).toString('hex');
    const txValidity = await writer.msb.state.getIndexerSequenceState();
    const payload = await applyStateMessageFactory(externalNode.msb.wallet, admin.config)
        .buildPartialBootstrapDeploymentMessage(
            externalNode.msb.wallet.address,
            externalBootstrap,
            randomBytes(32).toString('hex'),
            txValidity.toString('hex'),
            'json'
        );

    const rawPayload = await applyStateMessageFactory(writer.msb.wallet, admin.config)
        .buildCompleteBootstrapDeploymentMessage(
            payload.address,
            b4a.from(payload.bdo.tx, 'hex'),
            b4a.from(payload.bdo.txv, 'hex'),
            b4a.from(payload.bdo.bs, 'hex'),
            b4a.from(payload.bdo.ic, 'hex'),
            b4a.from(payload.bdo.in, 'hex'),
            b4a.from(payload.bdo.is, 'hex'),
        )
    await writer.msb.state.base.append(safeEncodeApplyOperation(rawPayload))
    await tick()
    await waitForHash(writer, payload.bdo.tx)
    return externalBootstrap
}

export const generatePostTx = async (writer, externalNode, externalContractBootstrap) => {
    const peerWriterKey = randomBytes(32).toString('hex');

    const testObj = {
        type: 'deployTest',
        value: {
            op: 'deploy',
            tick: Math.random().toString(),
            max: '21000000',
            lim: '1000',
            dec: 18
        }
    };

    const contentHash = await PeerWallet.blake3(JSON.stringify(testObj));
    const validity = await writer.msb.state.getIndexerSequenceState()
    const tx = await applyStateMessageFactory(externalNode.wallet, admin.config)
        .buildPartialTransactionOperationMessage(
            externalNode.wallet.address,
            peerWriterKey,
            b4a.toString(validity, 'hex'),
            b4a.toString(contentHash, 'hex'),
            externalContractBootstrap,
            b4a.toString(writer.msb.bootstrap, 'hex'),
            'json'
        )

    const postTxPayload = await applyStateMessageFactory(writer.wallet, admin.config)
        .buildCompleteTransactionOperationMessage(
            tx.address,
            b4a.from(tx.txo.tx, 'hex'),
            b4a.from(tx.txo.txv, 'hex'),
            b4a.from(tx.txo.iw, 'hex'),
            b4a.from(tx.txo.in, 'hex'),
            b4a.from(tx.txo.ch, 'hex'),
            b4a.from(tx.txo.is, 'hex'),
            b4a.from(tx.txo.bs, 'hex'),
            b4a.from(tx.txo.mbs, 'hex')
        );

    const postTx = safeEncodeApplyOperation(postTxPayload);
    return { postTx, txHash: tx.txo.tx };
}

/**
 * You can synchronize multiple nodes by passing them as arguments,
 * Useful for aligning signedLength values. If node is not a writer, it will be skipped.
 *
 * @example
 * await tryToSyncWriters(admin, writer1, writer2, indexer1);
 */
export const tryToSyncWriters = async (...args) => {
    try {
        const [first, ..._] = args
        await first.msb.state.base.forceFastForward()
        await first.msb.state.base.view.update()
        let attempts = 0;
        const maxAttempts = 10;
        while (attempts < maxAttempts) {
            let maxLength = Math.max(...args.map(node => node.msb.state.getSignedLength()));
            let allSynced = true;
            for (const node of args) {
                let signedLength = node.msb.state.getSignedLength();
                if (signedLength < maxLength) {
                    await node.msb.state.base.view.update();
                    await node.msb.network.swarm.flush()
                    await sleep(1000);
                    allSynced = false;
                }
            }
            if (allSynced) break;
            attempts++;
        }
    } catch (error) {
        throw new Error("Error synchronizing writers: " + error.message || error);
    }
}

export async function waitForNotIndexer(indexer) {
    try {
        let attempts = 0;
        const maxAttempts = 20;
        while (attempts < maxAttempts) {
            await indexer.msb.state.base.update();
            await indexer.msb.state.base.view.update();
            const indexersEntry = await indexer.msb.state.getIndexersEntry();
            let notIndexer = false;
            if (!indexersEntry) {
                notIndexer = true;
            } else {
                const formatted = formatIndexersEntry(indexersEntry, admin.config.addressLength);
                if (!formatted || !formatted.addresses) {
                    notIndexer = true;
                } else if (!formatted.addresses.includes(indexer.wallet.address)) {
                    notIndexer = true;
                }
            }
            if (notIndexer) {
                break;
            }
            await indexer.msb.network.swarm.flush()
            await sleep(1000);
            attempts++;
        }
    } catch (error) {
        throw new Error("Error waiting for indexer to not be an indexer: " + error.message || error);
    }
}

export async function waitIndexer(node, operation) {
    const waiter = new Promise(resolve => {
        node.msb.state.base.once(EventType.IS_INDEXER, (...args) => {
            resolve(args)
        })
    })
    await operation()
    return waiter
}

export async function waitWritable(admin, node, operation) {
    const waiter = new Promise(resolve => {
        node.msb.state.base.once(EventType.WRITABLE, (...args) => {
            resolve(args)
        })
    })
    await operation()
    await node.msb.state.base.view.update();
    await admin.msb.state.base.update()
    await node.msb.network.swarm.flush()
    return waiter
}

export async function waitDemotion(node, operation) {
    const waiter = new Promise(resolve => {
        node.msb.state.base.once(EventType.UNWRITABLE, (...args) => {
            resolve(args)
        })
    })
    await operation()
    return waiter
}

/**
 * Waits until the given node sees the expected state for a target address.
 *
 * @param {object} node - The node whose state will be checked (the observer).
 * @param {string} address - The address whose state we want to observe.
 * @param {object} expected - Object with properties: wk (Buffer), isWhitelisted (boolean), isWriter (boolean), isIndexer (boolean)
 * @returns {Promise<void>} Resolves when the observed state matches expected values.
 *
 * @example
 * await waitForRemoteNodeState(writer1, writer2.wallet.address, {
 *   wk: writer2.msb.state.writingKey,
 *   isWhitelisted: true,
 *   isWriter: true,
 *   isIndexer: false
 * });
 */

export async function waitForNodeState(node, address, expected) {
    try {
        await node.msb.state.base.flush()
        let attempts = 0;
        const maxAttempts = 50;
        while (attempts < maxAttempts) {
            const state = await node.msb.state.getNodeEntry(address);
            if (
                state &&
                b4a.equals(state.wk, expected.wk) &&
                state.isWhitelisted === (expected.isWhitelisted ?? expected.isWhiteListed) &&
                state.isWriter === expected.isWriter &&
                state.isIndexer === expected.isIndexer
            ) {
                return;
            }
            await node.msb.network.swarm.flush()
            await sleep(400);
            attempts++;
        }
    } catch (error) {
        throw new Error("Error synchronizing node state: " + error.message || error);
    }
}

export async function waitForHash(node, expected) {
    try {
        let attempts = 0;
        const maxAttempts = 20;
        while (attempts < maxAttempts) {
            const entry = await node.msb.state.get(expected);
            if (
                !!entry
            ) {
                break;
            }
            await node.msb.network.swarm.flush()
            await sleep(1000);
            attempts++;
        }
    } catch (error) {
        throw new Error('Error waiting for admin entry: ' + error.message || error);
    }
}

export async function waitForAdminEntry(node, expected) {
    try {
        let attempts = 0;
        const maxAttempts = 20;
        while (attempts < maxAttempts) {
            const adminEntry = await node.msb.state.getAdminEntry();
            if (
                adminEntry &&
                b4a.equals(adminEntry.wk, expected.wk) &&
                adminEntry.address === expected.address
            ) {
                break;
            }
            await node.msb.network.swarm.flush()
            await sleep(1000);
            attempts++;
        }
    } catch (error) {
        throw new Error('Error waiting for admin entry: ' + error.message || error);
    }
}

export async function waitForIndexersEntry(node, expected) {
    try {
        let attempts = 0;
        const maxAttempts = 20;
        while (attempts < maxAttempts) {
            const indexersEntry = await node.msb.state.getIndexersEntry();
            if (!indexersEntry) {
                attempts++;
                await sleep(250);
                continue;
            }
            const formatted = formatIndexersEntry(indexersEntry, admin.config.addressLength);
            if (formatted && formatted.addresses && formatted.addresses.includes(expected.address)) {
                break;
            }
            await sleep(250);
            attempts++;
        }
    } catch (error) {
        throw new Error("Error waiting for indexers entry: " + error.message || error);
    }
}
