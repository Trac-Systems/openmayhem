import { Config } from "./config.js";

export const ENV = {
  MAINNET: "mainnet",
  DEVELOPMENT: "development",
};
// TODO: CREATE TEST ENV CONFIG SIMILAR TO MAINNET AND USE IT IN TESTS.
// TODO: CREATE TESTNET1 ENV CONFIG and update npm scripts to run node witn mainnet or testnet1.

const configData = {
    [ENV.MAINNET]: {
        channel: "0000trac0network0peer0mainnet0000",
        storesDirectory: "stores/",
        storeName: "mainnet",
        txPoolMaxSize: 1_000,
        maxTxDelay: 60,
        maxMsbSignedLength: 1_000_000_000,
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
        maxMsbSignedLength: 1_000_000_000,
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
