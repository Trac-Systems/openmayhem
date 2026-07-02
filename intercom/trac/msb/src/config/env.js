import { TRAC_NETWORK_MSB_MAINNET_PREFIX } from 'trac-wallet/constants.js';
import { Config } from './config.js';

export const ENV = {
    MAINNET: 'mainnet',
    DEVELOPMENT: 'development',
    TESTNET1: 'testnet1'
}
// TODO: CREATE TEST ENV CONFIG SIMILAR TO MAINNET AND USE IT IN TESTS.
// TODO: CREATE TESTNET1 ENV CONFIG and update npm scripts to run node witn mainnet or testnet1.

const configData = {
    [ENV.MAINNET]: {
        addressLength: 63,
        addressPrefix: TRAC_NETWORK_MSB_MAINNET_PREFIX,
        addressPrefixLength: TRAC_NETWORK_MSB_MAINNET_PREFIX.length,
        bech32mHrpLength: TRAC_NETWORK_MSB_MAINNET_PREFIX.length + 1, // len(addressPrefix + separator)
        bootstrap: 'acbc3a4344d3a804101d40e53db1dda82b767646425af73599d4cd6577d69685',
        channel: '0000trac0network0msb0mainnet0000',
        dhtBootstrap: ['116.202.214.149:10001', '157.180.12.214:10001', 'node1.hyperdht.org:49737', 'node2.hyperdht.org:49737', 'node3.hyperdht.org:49737'],
        disableRateLimit: false,
        enableErrorApplyLogs: false,
        enableInteractiveMode: true,
        enableRoleRequester: false,
        enableTxApplyLogs: false,
        enableValidatorObserver: true,
        enableWallet: true,
        maxValidators: 50,
        maxRetries: 3,
        messageThreshold: 3,
        messageValidatorRetryDelay: 1000, //How long to wait before retrying (ms) MESSAGE_VALIDATOR_RETRY_DELAY_MS
        messageValidatorResponseTimeout: 3 * 3 * 1000, //Overall timeout for sending a message (ms). This is 3 * maxRetries * messageValidatorRetryDelay;
        networkId: 918,
        storesDirectory: 'stores/',
    },
    [ENV.DEVELOPMENT]: {
        addressLength: 63,
        addressPrefix: TRAC_NETWORK_MSB_MAINNET_PREFIX,
        addressPrefixLength: TRAC_NETWORK_MSB_MAINNET_PREFIX.length,
        bech32mHrpLength: TRAC_NETWORK_MSB_MAINNET_PREFIX.length + 1, // len(addressPrefix + separator)
        bootstrap: 'e90cca53847a12a82f3bf0f67401e45e2ccc1698ee163e61414c2894eb3c6b12',
        channel: '12312313123123',
        dhtBootstrap: ['116.202.214.149:10001', '157.180.12.214:10001', 'node1.hyperdht.org:49737', 'node2.hyperdht.org:49737', 'node3.hyperdht.org:49737'],
        disableRateLimit: false,
        enableErrorApplyLogs: true,
        enableInteractiveMode: true,
        enableRoleRequester: false,
        enableTxApplyLogs: true,
        enableValidatorObserver: true,
        enableWallet: true,
        maxValidators: 6,
        maxRetries: 0,
        messageThreshold: 1000,
        messageValidatorRetryDelay: 1000, //How long to wait before retrying (ms) MESSAGE_VALIDATOR_RETRY_DELAY_MS
        messageValidatorResponseTimeout: 3 * 3 * 1000, //Overall timeout for sending a message (ms). This is 3 * maxRetries * messageValidatorRetryDelay;
        networkId: 918,
        storesDirectory : 'stores/',
    }
}

export const createConfig = (environment, options) => {
    return new Config(options, configData[environment])
}
