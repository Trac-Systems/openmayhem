import { Config } from "./config.js";

export const ENV = {
  MAINNET: "mainnet",
  DEVELOPMENT: "development",
  TESTNET1: "testnet1",
};
// TODO: CREATE TEST ENV CONFIG SIMILAR TO MAINNET AND USE IT IN TESTS.

const configData = {
    [ENV.MAINNET]: {
        channel: "0000trac0network0peer0mainnet0000",
        storesDirectory: "stores/",
        storeName: "mainnet",
        txPoolMaxSize: 1_000,
        maxTxDelay: 60,
        hyperbeeCacheMaxEntries: 65_536,
        maxMsbSignedLength: 1_000_000_000,
        maxMsbSignedLengthFutureDelta: 1_000_000,
        maxMsbApplyOperationBytes: 1024 * 1024,
        enableInteractiveMode: true,
        enableBackgroundTasks: true,
        enableUpdater: true,
        replicate: true,
        dhtBootstrap: [
            "116.202.214.149:10001",
            "157.180.12.214:10001",
            "node1.hyperdht.org:49737",
            "node2.hyperdht.org:49737",
            "node3.hyperdht.org:49737",
        ],
        enableTxlogs: false,
        apiTxExposed: false,
        apiMsgExposed: false,
        bootstrap: null,
    },
    [ENV.TESTNET1]: {
        channel: "1111trac1network1peer1testnet1111",
        storesDirectory: "stores/",
        storeName: "testnet",
        txPoolMaxSize: 1_000,
        maxTxDelay: 60,
        hyperbeeCacheMaxEntries: 65_536,
        maxMsbSignedLength: 1_000_000_000,
        maxMsbSignedLengthFutureDelta: 1_000_000,
        maxMsbApplyOperationBytes: 1024 * 1024,
        enableInteractiveMode: true,
        enableBackgroundTasks: true,
        enableUpdater: true,
        replicate: true,
        dhtBootstrap: [
            "116.202.214.149:10001",
            "157.180.12.214:10001",
            "node1.hyperdht.org:49737",
            "node2.hyperdht.org:49737",
            "node3.hyperdht.org:49737",
        ],
        enableTxlogs: false,
        apiTxExposed: false,
        apiMsgExposed: false,
        bootstrap: null,
    },
    [ENV.DEVELOPMENT]: {
        channel: "unit-test",
        storesDirectory: "stores/",
        storeName: "peer",
        txPoolMaxSize: 1_000,
        maxTxDelay: 60,
        hyperbeeCacheMaxEntries: 65_536,
        maxMsbSignedLength: 1_000_000_000,
        maxMsbSignedLengthFutureDelta: 1_000_000,
        maxMsbApplyOperationBytes: 1024 * 1024,
        enableInteractiveMode: true,
        enableBackgroundTasks: false,
        enableUpdater: false,
        replicate: false,
        dhtBootstrap: [
            "116.202.214.149:10001",
            "157.180.12.214:10001",
            "node1.hyperdht.org:49737",
            "node2.hyperdht.org:49737",
            "node3.hyperdht.org:49737",
        ],
        enableTxlogs: false,
        apiTxExposed: false,
        apiMsgExposed: false,
        bootstrap: null,
    }
};

export const createConfig = (environment, options = {}) => {
  return new Config(options, configData[environment]);
};
