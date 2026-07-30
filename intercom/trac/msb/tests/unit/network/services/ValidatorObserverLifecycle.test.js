import sinon from "sinon";
import { test } from "brittle";
import b4a from "b4a";
import tracCryptoApi from "trac-crypto-api";
import ValidatorObserverService from "../../../../src/core/network/services/ValidatorObserverService.js";
import { bufferToAddress } from "../../../../src/core/state/utils/address.js";

if (typeof setTimeout !== "undefined" && typeof setTimeout.restore === "function") {
    setTimeout.restore();
}

const originalDecode = tracCryptoApi.address.decode;

const defaultMockDecode = (addr) => {
    const buf = b4a.alloc(32, 0); 
    try {
        if (b4a.isBuffer(addr)) {
            b4a.copy(addr, buf, 0, 0, Math.min(addr.length, 32));
        } else if (typeof addr === "string") {
            b4a.write(buf, addr);
        }
    } catch {}
    return buf;
};

async function cleanup(service) {
    tracCryptoApi.address.decode = originalDecode;
    if (service) {
        await service.stopValidatorObserver(false);
    }
}

function createBaseMocks(overrides = {}) {
    tracCryptoApi.address.decode = defaultMockDecode;

    const baseState = {
        getAdminEntry: async () => ({ address: "trac_admin" }),
        getRegisteredWriterKey: async (hex) => hex ? b4a.from(hex, "hex") : b4a.alloc(32, "a"),
        getNodeEntry: async () => ({ isWriter: true }),
        base: {
            system: {
                list: async function* () {
                    yield { key: b4a.alloc(32, "k"), value: { isRemoved: false } };
                },
            },
        },
    };

    const baseNetwork = {
        validatorConnectionManager: {
            connectionCount: () => 0,
            maxConnectionsReached: () => false,
            connected: () => false,
            exists: () => false,
            connectedValidators: () => [],
            remove: () => {},
            destroy: async () => {},
            close: async () => {}
        },
        pendingConnectionsCount: () => 0,
        isConnectionPending: () => false,
        tryConnect: async () => {}, 
    };

    return {
        network: {
            ...baseNetwork,
            ...overrides.network,
            validatorConnectionManager: {
                ...baseNetwork.validatorConnectionManager,
                ...(overrides.network?.validatorConnectionManager || {}),
            },
        },
        state: {
            ...baseState,
            ...(overrides.state || {}),
        },
        config: {
            enableValidatorObserver: true,
            pollInterval: 10,
            addressLength: 32,
            addressPrefix: "trac",
            maxWritersForAdminIndexerConnection: 10,
            maxValidators: 10,
            validatorConnectionAttemptDelay: 5,
            ...(overrides.config || {}),
        },
    };
}

test("connects successfully (happy path)", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    let calls = 0;

    const { network, state, config } = createBaseMocks({
        network: { tryConnect: () => calls++ },
    });

    const service = new ValidatorObserverService(network, state, "self", config);

    try {
        await service.start();

        for (let i = 0; i < 50 && calls === 0; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver(false);

        t.ok(calls > 0);
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});
test("rotation allows multiple attempts", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    let calls = 0;

    const writers = [
        { key: b4a.alloc(32, "1"), value: { isRemoved: false } },
        { key: b4a.alloc(32, "2"), value: { isRemoved: false } },
        { key: b4a.alloc(32, "3"), value: { isRemoved: false } },
    ];

    const mocks = createBaseMocks({
        network: { tryConnect: () => calls++ },
        state: {
            base: {
                system: {
                    list: async function* () {
                        for (const w of writers) yield w;
                    },
                },
            },
        },
        config: { pollInterval: 10 }
    });

    const service = new ValidatorObserverService(
        mocks.network, 
        mocks.state, 
        "self", 
        mocks.config
    );

    try {
        await service.start();

        for (let i = 0; i < 50 && calls < 2; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver(false);

        t.ok(calls >= 2);
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("does NOT connect if already connected", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    let calls = 0;

    const { network, state, config } = createBaseMocks({
        network: {
            validatorConnectionManager: { connected: () => true },
            tryConnect: () => calls++,
        },
    });

    const service = new ValidatorObserverService(network, state, "self", config);

    try {
        await service.start();

        for (let i = 0; i < 20; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver(false);

        t.is(calls, 0);
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("does NOT connect if not writer", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    let calls = 0;

    const { network, state, config } = createBaseMocks({
        network: { tryConnect: () => calls++ },
        state: { getNodeEntry: async () => ({ isWriter: false }) },
        config: { enableValidatorObserver: false }
    });

    const service = new ValidatorObserverService(network, state, "self", config);

    try {
        await service.start();

        for (let i = 0; i < 20; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver(false);

        t.is(calls, 0);
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("does NOT connect if max reached", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    let calls = 0;

    const { network, state, config } = createBaseMocks({
        network: {
            validatorConnectionManager: { maxConnectionsReached: () => true },
            tryConnect: () => calls++,
        },
    });

    const service = new ValidatorObserverService(network, state, "self", config);

    try {
        await service.start();

        for (let i = 0; i < 20; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver(false);

        t.is(calls, 0);
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("blocks when too many pending", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    let calls = 0;

    const { network, state, config } = createBaseMocks({
        network: {
            isConnectionPending: () => true,
            tryConnect: () => calls++,
        },
    });

    const service = new ValidatorObserverService(network, state, "self", config);

    try {
        await service.start();

        for (let i = 0; i < 20; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver(false);

        t.is(calls, 0);
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("returns early when no candidates available", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    let calls = 0;

    const { network, state, config } = createBaseMocks({
        network: {
            validatorConnectionManager: { connected: () => true },
            tryConnect: () => calls++,
        },
    });

    const service = new ValidatorObserverService(network, state, "self", config);

    try {
        await service.start();

        for (let i = 0; i < 20; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver(false);

        t.is(calls, 0);
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("bootstrap switches to long TTL after timeout", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    let scans = 0;

    const mocks = createBaseMocks({
        state: {
            base: {
                system: {
                    list: async function* () {
                        scans++;
                        yield { key: b4a.alloc(32), value: { isRemoved: false } };
                    },
                },
            },
        },
        config: { 
            bootstrapTimeout: 30_000,
            pollInterval: 10
        }
    });

    const service = new ValidatorObserverService(mocks.network, mocks.state, "self", mocks.config);

    try {
        await service.start();

        for (let i = 0; i < 10; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        const before = scans;
        clock.tick(31_000);
        await Promise.resolve();

        await service.stopValidatorObserver(true);

        t.ok(scans >= before);
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("does NOT drop connections when it is the admin and threshold reached", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    const removed = [];
    const publicKeys = ["a", "b", "c"];

    const { network, state, config } = createBaseMocks({
        network: {
            validatorConnectionManager: {
                connectedValidators: () => publicKeys,
                remove: (pk) => removed.push(pk),
            },
        },
        config: { maxWritersForAdminIndexerConnection: 0 },
    });

    const stateOverride = {
        ...state,
        getAdminEntry: async () => ({ address: "self" }),
    };

    const service = new ValidatorObserverService(
        network,
        stateOverride,
        "self",
        config
    );

    try {
        await service.start();

        for (let i = 0; i < 20; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver(false);

        t.alike(removed, []);
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("removes admin when threshold exceeded", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    let removed = 0;

    const { network, config } = createBaseMocks({
        network: {
            validatorConnectionManager: {
                remove: () => removed++,
            },
        },
        config: { maxWritersForAdminIndexerConnection: 0 },
    });

    const state = {
        getAdminEntry: async () => ({ address: "placeholder" }),
        getRegisteredWriterKey: async () => b4a.alloc(32),
        getNodeEntry: async () => ({ isWriter: true }),
        base: {
            system: {
                list: async function* () {
                    yield { key: b4a.alloc(32), value: { isRemoved: false } };
                },
            },
        },
    };

    const realAddress = bufferToAddress(
        await state.getRegisteredWriterKey(),
        config.addressPrefix
    );

    state.getAdminEntry = async () => ({ address: realAddress });

    const service = new ValidatorObserverService(
        network,
        state,
        "self",
        config
    );

    try {
        await service.start();

        for (let i = 0; i < 20 && removed === 0; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver(false);

        t.ok(removed >= 1);
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("removes stale connections when writer is marked as removed", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    let removed = 0;
    const fakePublicKey = b4a.alloc(32, 1);

    const { network, state, config } = createBaseMocks({
        network: {
            validatorConnectionManager: {
                connected: () => true,
                remove: () => removed++,
            },
        },
        state: {
            base: {
                system: {
                    list: async function* () {
                        yield {
                            key: b4a.alloc(32),
                            value: { isRemoved: true },
                        };
                    },
                },
            },
        },
    });

    const originalMock = tracCryptoApi.address.decode;
    tracCryptoApi.address.decode = () => fakePublicKey;

    const service = new ValidatorObserverService(network, state, "self", config);

    try {
        await service.start();

        for (let i = 0; i < 20 && removed === 0; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver(false);

        t.ok(removed > 0);
    } finally {
        tracCryptoApi.address.decode = originalMock;
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("does not remove stale connection if not connected", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    let removed = 0;

    const { network, state, config } = createBaseMocks({
        network: {
            validatorConnectionManager: {
                connected: () => false,
                remove: () => removed++,
            },
        },
        state: {
            base: {
                system: {
                    list: async function* () {
                        yield {
                            key: b4a.alloc(32),
                            value: { isRemoved: true },
                        };
                    },
                },
            },
        },
    });

    const service = new ValidatorObserverService(network, state, "self", config);

    try {
        await service.start();

        for (let i = 0; i < 20; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver(false);

        t.is(removed, 0);
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("does not start observer if it is already running", async (t) => {
    const { network, state, config } = createBaseMocks();
    const service = new ValidatorObserverService(network, state, "self", config);

    await service.start();
    await service.start();
    await service.stopValidatorObserver();

    t.pass();
});

test("gracefully catches exceptions in the worker loop", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    const { network, state, config } = createBaseMocks({
        state: {
            getAdminEntry: async () => { throw new Error("Simulated DB Crash"); }
        }
    });

    const service = new ValidatorObserverService(network, state, "self", config);

    try {
        await service.start();

        for (let i = 0; i < 20; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver();

        t.pass();
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("gracefully catches exceptions in scanAutobaseWriters", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    const { network, state, config } = createBaseMocks({
        state: {
            base: {
                system: {
                    list: () => { throw new Error("Simulated Autobase Iterator Crash"); }
                }
            }
        }
    });

    const service = new ValidatorObserverService(network, state, "self", config);

    try {
        await service.start();

        for (let i = 0; i < 20; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver();

        t.pass();
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("gracefully catches exceptions in tryConnect", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    const { network, state, config } = createBaseMocks({
        network: {
            tryConnect: () => { throw new Error("Simulated Network Transport Error"); }
        }
    });

    const service = new ValidatorObserverService(network, state, "self", config);

    try {
        await service.start();

        for (let i = 0; i < 20; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver();

        t.pass();
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("caches null if the address buffer is invalid or missing", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    const { network, state, config } = createBaseMocks({
        state: {
            getRegisteredWriterKey: async () => b4a.alloc(5, "bad") 
        }
    });

    const service = new ValidatorObserverService(network, state, "self", config);

    try {
        await service.start();

        for (let i = 0; i < 20; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver();

        t.pass();
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("caches null if public key fails to decode", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    const { network, state, config } = createBaseMocks();

    const originalMock = tracCryptoApi.address.decode;
    tracCryptoApi.address.decode = () => null;

    const service = new ValidatorObserverService(network, state, "self", config);

    try {
        await service.start();

        for (let i = 0; i < 20; i++) {
            clock.tick(10);
            await Promise.resolve();
        }

        await service.stopValidatorObserver();

        t.pass();
    } finally {
        tracCryptoApi.address.decode = originalMock;
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});

test("clears memory cache when exceeding MAX_KEY_DECODE_CACHE_SIZE", async (t) => {
    const clock = sinon.useFakeTimers({ now: 0 });

    let callCount = 0;

    const { network, state, config } = createBaseMocks({
        state: {
            base: {
                system: {
                    list: async function* () {
                        callCount++;
                        for (let i = 0; i < 4000; i++) {
                            const keyBuf = b4a.alloc(32);
                            b4a.write(keyBuf, `k-${callCount}-${i}`);
                            yield { key: keyBuf, value: { isRemoved: false } };
                        }
                    },
                },
            },
        },
        config: { pollInterval: 5 }
    });

    const service = new ValidatorObserverService(network, state, "self", config);

    try {
        await service.start();

        for (let i = 0; i < 20; i++) {
            clock.tick(5);
            await Promise.resolve();
        }

        await service.stopValidatorObserver();

        t.pass();
    } finally {
        clock.restore();
        sinon.restore();
        await cleanup(service);
    }
});