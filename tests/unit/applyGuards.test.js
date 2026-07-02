import test from 'brittle';
import b4a from 'b4a';
import path from 'path';
import os from 'os';
import fs from 'fs/promises';
import PeerWallet from 'trac-wallet';
import { safeEncodeApplyOperation } from 'trac-msb/src/utils/protobuf/operationHelpers.js';
import { blake3 } from '@tracsystems/blake3'
import { Peer, createConfig, ENV } from '../../src/index.js';
import Wallet from '../../src/wallet.js';
import { mkdtempPortable, rmrfPortable } from '../helpers/tmpdir.js';

class TestProtocol {
    constructor(peer, base, config) {
    }
    async extendApi() {}
    getError(value) {
        return value ?? null;
    }
    txMaxBytes() {
        return 1024 * 1024;
    }
    msgMaxBytes() {
        return 1024 * 1024;
    }
    featMaxBytes() {
        return 1024 * 1024;
    }
}

class TestContract {
    constructor(protocol) {}
    async execute(op, batch) {
        return null;
    }
}

async function withTempDir(fn) {
    const tmpRoot = await mkdtempPortable(path.join(os.tmpdir(), 'trac-peer-tests-'));
    const storesDirectory = tmpRoot.endsWith(path.sep) ? tmpRoot : tmpRoot + path.sep;
    try {
        return await fn({ tmpRoot, storesDirectory });
    } finally {
        await rmrfPortable(tmpRoot);
    }
}

async function closePeer(peer) {
    if (!peer) return;
    try {
        await peer.close();
    } catch (_e) {}
    try {
        await peer.store.close();
    } catch (_e) {}
}

async function prepareWallet(storesDirectory, storeName) {
    const wallet = new Wallet();
    await wallet.generateKeyPair();
    const keypairPath = path.join(storesDirectory, storeName, 'db', 'keypair.json');
    await fs.mkdir(path.dirname(keypairPath), { recursive: true });
    await wallet.exportToFile(keypairPath, b4a.alloc(0));
    return wallet;
}

function makeHex32(fill) {
    return b4a.alloc(32).fill(fill).toString('hex');
}

function makeMsbStub({ msbBootstrapBuf, signedLength, getEntry }) {
    const core = {
        signedLength,
        once(event, cb) {
            // Used only if apply has to wait; tests will override if needed.
        },
    };

    return {
        async ready() {},
        config: {
            bootstrap: msbBootstrapBuf,
            addressPrefix: 'trac',
            networkId: 918,
        },
        network: {},
        state: {
            base: {
                view: {
                    core,
                    checkout(_signedLength) {
                        return {
                            async get(key) {
                                return await getEntry(key, _signedLength);
                            },
                            async close() {},
                        };
                    },
                },
            },
            getSignedLength() {
                return core.signedLength;
            },
        },
    };
}

test('apply: tx msbsl stall guard skips waiting', async (t) => {
    await withTempDir(async ({ storesDirectory }) => {
        const msbBootstrapBuf = b4a.alloc(32).fill(7);
        const msb = makeMsbStub({
            msbBootstrapBuf,
            signedLength: 0,
            async getEntry() {
                return null;
            },
        });

        // If the code ever tries to wait for msbCore.append, fail fast.
        msb.state.base.view.core.once = () => {
            throw new Error('apply should not wait for msbCore.append in this test');
        };

        const storeName = 'peer-stall-guard';
        const wallet = await prepareWallet(storesDirectory, storeName);
        const config = createConfig(ENV.DEVELOPMENT, {
            storesDirectory,
            storeName,
            maxMsbSignedLength: 10,
        });
        const peer = new Peer({
            config,
            msb,
            protocol: TestProtocol,
            contract: TestContract,
            wallet,
        });

        try {
            await peer.ready();

            const txHashHex = makeHex32(1);
            const op = {
                type: 'tx',
                key: txHashHex,
                value: {
                    dispatch: { type: 'ping', value: { msg: 'hi' } },
                    msbsl: 100, // above maxMsbSignedLength => should be skipped before any waiting
                    ipk: makeHex32(2),
                    wp: makeHex32(3),
                },
            };

            const timeout = new Promise((_, reject) =>
                setTimeout(() => reject(new Error('append timed out (possible apply stall)')), 2000)
            );
            await Promise.race([peer.base.append(op), timeout]);

            const txl = await peer.bee.get('txl');
            t.is(txl, null, 'tx should not be indexed when msbsl exceeds maxMsbSignedLength');
        } finally {
            await closePeer(peer);
        }
    });
});

test('apply: tx MSB payload size guard blocks otherwise-valid tx', async (t) => {
    await withTempDir(async ({ storesDirectory }) => {
        const msbBootstrapBuf = b4a.alloc(32).fill(7);

        const txHashHex = makeHex32(1);
        const ipkHex = makeHex32(2);
        const wpHex = makeHex32(3);
        const invokerAddress = PeerWallet.encodeBech32mSafe('trac', b4a.from(ipkHex, 'hex'));
        const validatorAddress = PeerWallet.encodeBech32mSafe('trac', b4a.from(wpHex, 'hex'));

        const dispatch = { type: 'ping', value: { msg: 'hello' } };
        const ch = b4a.toString(await blake3(JSON.stringify(dispatch)), 'hex');

        const makePeer = async (maxBytes, storeName) => {
            let msbOperation = null;
            const msb = makeMsbStub({
                msbBootstrapBuf,
                signedLength: 1,
                async getEntry(key, _signedLength) {
                    if (key !== txHashHex) return null;
                    if (!msbOperation) return null;
                    return { value: msbOperation };
                },
            });

            const wallet = await prepareWallet(storesDirectory, storeName);
            const config = createConfig(ENV.DEVELOPMENT, {
                storesDirectory,
                storeName,
                maxMsbApplyOperationBytes: maxBytes,
            });
            const peer = new Peer({
                config,
                msb,
                protocol: TestProtocol,
                contract: TestContract,
                wallet,
            });
            await peer.ready();

            // Bind an MSB operation that matches this specific peer's subnet bootstrap.
            const subnetBootstrapBuf = peer.config.bootstrap;
            msbOperation = safeEncodeApplyOperation({
                type: 12,
                address: b4a.from(invokerAddress, 'ascii'),
                txo: {
                    tx: b4a.from(txHashHex, 'hex'),
                    bs: subnetBootstrapBuf,
                    mbs: msbBootstrapBuf,
                    ch: b4a.from(ch, 'hex'),
                    va: b4a.from(validatorAddress, 'ascii'),
                },
            });

            t.ok(msbOperation.byteLength > 0, 'fixture MSB operation encodes');
            return peer;
        };

        const op = {
            type: 'tx',
            key: txHashHex,
            value: {
                dispatch,
                msbsl: 1,
                ipk: ipkHex,
                wp: wpHex,
            },
        };

        const peerBlocked = await makePeer(1, 'peer-maxbytes-blocked');
        try {
            const maxBytes = peerBlocked.config.maxMsbApplyOperationBytes;
            const msbLen = (await peerBlocked.msbClient.getSignedAtLength(txHashHex, 1))?.value?.byteLength ?? null;
            t.ok(msbLen !== null && msbLen > maxBytes, 'fixture MSB payload is larger than max bytes');
            await peerBlocked.base.append(op);
            await peerBlocked.base.update();
            const txl = await peerBlocked.bee.get('txl');
            t.is(txl, null, 'tx should not be indexed when MSB apply payload exceeds max bytes');
        } finally {
            await closePeer(peerBlocked);
        }

        const peerAllowed = await makePeer(1024 * 1024, 'peer-maxbytes-allowed');
        try {
            await peerAllowed.base.append(op);
            await peerAllowed.base.update();
            const txl = await peerAllowed.bee.get('txl');
            t.is(txl?.value ?? null, 1, 'tx should be indexed when MSB apply payload is within max bytes');
        } finally {
            await closePeer(peerAllowed);
        }
    });
});
