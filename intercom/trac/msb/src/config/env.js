import { TRAC_NETWORK_MSB_MAINNET_PREFIX, TRAC_NETWORK_MSB_TESTNET1_PREFIX } from 'trac-wallet';
import { TRAC_NETWORK_TESTNET_ID, TRAC_NETWORK_MAINNET_ID } from 'trac-crypto-api/constants.js';
import { Config } from './config.js';
import { address } from 'trac-crypto-api';

export const ENV = {
    MAINNET: 'mainnet',
    DEVELOPMENT: 'development',
    TESTNET1: 'testnet1'
}

const configData = {
    [ENV.TESTNET1]: {
        debug: false,
        addressLength: 67,
        addressPrefix: TRAC_NETWORK_MSB_TESTNET1_PREFIX,
        addressPrefixLength: TRAC_NETWORK_MSB_TESTNET1_PREFIX.length,
        bech32mHrpLength: TRAC_NETWORK_MSB_TESTNET1_PREFIX.length + 1, // len(addressPrefix + separator)
        bootstrap: 'c184f4ad8e9cf5e911f9415b60e7dcfb30aed73ebd8a402ef68e1b154624f5ef',
        channel: '1111trac1network1msb1testnet1111',
        dhtBootstrap: ['116.202.214.149:10001','157.180.12.214:10001','node1.hyperdht.org:49737','node2.hyperdht.org:49737','node3.hyperdht.org:49737'], // these are used to peer discovery
        derivationPath: address.TESNET_DERIVATION_PATH,
        enableValidatorObserver: true,
        pollInterval: 500, // Validator observer poll interval
        adminCacheTTL: 10_000, // Admin cache TTL ms
        validatorConnectionAttemptDelay: 5, // Delay between validator connection attempts (ms)
        bootstrapTimeout: 60_000, // time used (ms) to connect to new validators at bootstrap
        writersShortCacheTTL: 10_000, // short TTL during bootstrap
        writersLongCacheTTL: 120_000, // long TTL after bootstrap
        maxValidators: 50,
        maxWritersForAdminIndexerConnection: 10, // Connectivity constants
        disableRateLimit: false,
        enableErrorApplyLogs: true,
        enableInteractiveMode: true,
        enableRoleRequester: false,
        enableTxApplyLogs: true,
        enableWallet: true,
        hyperbeeCacheMaxEntries: 65_536,
        maxRetries: 3,
        messageThreshold: 3,
        messageValidatorRetryDelay: 1000, //How long to wait before retrying (ms) MESSAGE_VALIDATOR_RETRY_DELAY_MS
        messageValidatorResponseTimeout: 3 * 3 * 1000, //Overall timeout for sending a message (ms). This is 3 * maxRetries * messageValidatorRetryDelay;
        host: 'localhost',
        port: 5000,
        networkId: TRAC_NETWORK_TESTNET_ID,
        maxPeers: 64, // Connectivity constants
        maxParallel: 64, // Connectivity constants
        maxServerConnections: Infinity, // Connectivity constants
        maxClientConnections: Infinity, // Connectivity constants
        processIntervalMs: 50, // Pool constants
        transactionPoolSize: 1000, // Operation handler constants
        rateLimitCleanupIntervalMs: 120_000, // Rate limiting constants
        rateLimitConnectionTimeoutMs: 60_000, // Rate limiting constants
        rateLimitMaxTransactionsPerSecond: 50, // Rate limiting constants
        maxPendingRequestsInPendingRequestsService: 50_000, // Maximum number of pending requests in PendingRequestService (This value should not exceed 256MB)
        pendingRequestTimeout: 3000, // constant after which time the transaction will be considered invalid
        txCommitTimeout: 2200,
        txPoolSize: 1000, // size of transaction pool
        validatorHealthCheckInterval: 5 * 60 * 1000, // How often to check validator health (ms)
        storesDirectory: 'stores/',
        storeName: 'testnet',
    },
    [ENV.MAINNET]: {
        debug: false,
        addressLength: 63,
        addressPrefix: TRAC_NETWORK_MSB_MAINNET_PREFIX,
        addressPrefixLength: TRAC_NETWORK_MSB_MAINNET_PREFIX.length,
        bech32mHrpLength: TRAC_NETWORK_MSB_MAINNET_PREFIX.length + 1, // len(addressPrefix + separator)
        bootstrap: 'acbc3a4344d3a804101d40e53db1dda82b767646425af73599d4cd6577d69685',
        channel: '0000trac0network0msb0mainnet0000',
        dhtBootstrap: ['116.202.214.149:10001','157.180.12.214:10001','node1.hyperdht.org:49737','node2.hyperdht.org:49737','node3.hyperdht.org:49737'],
        derivationPath: address.MAINNET_DERIVATION_PATH,
        enableValidatorObserver: true,
        pollInterval: 500, // Validator observer poll interval
        adminCacheTTL: 3_600_000, // Admin cache TTL ms
        validatorConnectionAttemptDelay: 5, // Delay between validator connection attempts (ms)
        bootstrapTimeout: 120_000, // time used (ms) to connect to new validators at bootstrap
        writersShortCacheTTL: 2_000, // short TTL during bootstrap
        writersLongCacheTTL: 120_000, // long TTL after bootstrap
        maxValidators: 50,
        maxWritersForAdminIndexerConnection: 10, // Connectivity constants
        disableRateLimit: false,
        enableErrorApplyLogs: false,
        enableInteractiveMode: true,
        enableRoleRequester: false,
        enableTxApplyLogs: false,
        enableWallet: true,
        hyperbeeCacheMaxEntries: 65_536,
        maxRetries: 3,
        messageThreshold: 3,
        messageValidatorRetryDelay: 1000, //How long to wait before retrying (ms) MESSAGE_VALIDATOR_RETRY_DELAY_MS
        messageValidatorResponseTimeout: 3 * 3 * 1000, //Overall timeout for sending a message (ms). This is 3 * maxRetries * messageValidatorRetryDelay;
        host: 'localhost',
        port: 5000,
        networkId: TRAC_NETWORK_MAINNET_ID,
        maxPeers: 64, // Connectivity constants
        maxParallel: 64, // Connectivity constants
        maxServerConnections: Infinity, // Connectivity constants
        maxClientConnections: Infinity, // Connectivity constants
        processIntervalMs: 50, // Pool constants
        transactionPoolSize: 1000, // Operation handler constants
        rateLimitCleanupIntervalMs: 120_000, // Rate limiting constants
        rateLimitConnectionTimeoutMs: 60_000, // Rate limiting constants
        rateLimitMaxTransactionsPerSecond: 50, // Rate limiting constants
        maxPendingRequestsInPendingRequestsService: 50_000, // Maximum number of pending requests in PendingRequestService (This value should not exceed 256MB)
        pendingRequestTimeout: 3000, // constant after which time the transaction will be considered invalid
        txCommitTimeout: 2200,
        txPoolSize: 1000, // size of transaction pool
        validatorHealthCheckInterval: 5 * 60 * 1000, // How often to check validator health (ms)
        storesDirectory: 'stores/',
        storeName: 'mainnet',
    },
    [ENV.DEVELOPMENT]: {
        debug: false,
        addressLength: 63,
        addressPrefix: TRAC_NETWORK_MSB_MAINNET_PREFIX,
        addressPrefixLength: TRAC_NETWORK_MSB_MAINNET_PREFIX.length,
        bech32mHrpLength: TRAC_NETWORK_MSB_MAINNET_PREFIX.length + 1, // len(addressPrefix + separator)
        bootstrap: '12f7f1668eac2e691e17cbc6a53e509c5cee78cdcac562313091c64e5fd077d6',
        channel: '12312313123123',
        dhtBootstrap: ['116.202.214.149:10001','157.180.12.214:10001','node1.hyperdht.org:49737','node2.hyperdht.org:49737','node3.hyperdht.org:49737'],
        derivationPath: address.MAINNET_DERIVATION_PATH,
        enableValidatorObserver: true,
        pollInterval: 500, // Validator observer poll interval
        adminCacheTTL: 60_000, // Admin cache TTL ms
        validatorConnectionAttemptDelay: 5, // Delay between validator connection attempts (ms)
        bootstrapTimeout: 30_000,  // time used (ms) to connect to new validators at bootstrap
        writersShortCacheTTL: 1_000, // short TTL during bootstrap
        writersLongCacheTTL: 2_000, // long TTL after bootstrap
        maxValidators: 10,
        maxWritersForAdminIndexerConnection: 10, // Connectivity constants
        disableRateLimit: false,
        enableErrorApplyLogs: true,
        enableInteractiveMode: true,
        enableRoleRequester: false,
        enableTxApplyLogs: true,
        enableWallet: true,
        hyperbeeCacheMaxEntries: 65_536,
        maxRetries: 3,
        messageThreshold: 1000,
        messageValidatorRetryDelay: 1000, //How long to wait before retrying (ms) MESSAGE_VALIDATOR_RETRY_DELAY_MS
        messageValidatorResponseTimeout: 3 * 3 * 1000, //Overall timeout for sending a message (ms). This is 3 * maxRetries * messageValidatorRetryDelay;
        host: 'localhost',
        port: 5000,
        networkId: TRAC_NETWORK_MAINNET_ID,
        maxPeers: 64, // Connectivity constants
        maxParallel: 64, // Connectivity constants
        maxServerConnections: Infinity, // Connectivity constants
        maxClientConnections: Infinity, // Connectivity constants
        processIntervalMs: 50, // Pool constants
        transactionPoolSize: 1000, // Operation handler constants
        rateLimitCleanupIntervalMs: 120_000, // Rate limiting constants
        rateLimitConnectionTimeoutMs: 60_000, // Rate limiting constants
        rateLimitMaxTransactionsPerSecond: 50, // Rate limiting constants
        maxPendingRequestsInPendingRequestsService: 50_000, // Maximum number of pending requests in PendingRequestService (This value should not exceed 256MB)
        pendingRequestTimeout: 3000, // constant after which time the transaction will be considered invalid
        txCommitTimeout: 2200,
        txPoolSize: 1000, // size of transaction pool
        validatorHealthCheckInterval: 1_000, // How often to check validator health (ms)
        storesDirectory: 'stores/',
        storeName: 'development',
    }
}

export const createConfig = (environment, options) => {
    return new Config(options, configData[environment])
}
