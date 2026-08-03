import b4a from 'b4a'
import { isDefined } from '../utils/type.js'
import _ from 'lodash'

const DEFAULT_HYPERBEE_CACHE_MAX_ENTRIES = 65_536

export class Config {
    #options
    #config
    #bootstrap
    #channel

    constructor(options = {}, config = {}) {
        this.#validate(options, config)
        this.#options = options
        this.#config = config
        this.#bootstrap = this.#normalizeBootstrap(
            isDefined(options.bootstrap) ? options.bootstrap : config.bootstrap
        )
        this.#channel = this.#normalizeChannel(
            isDefined(options.channel) ? options.channel : config.channel
        )
        void this.hyperbeeCacheMaxEntries
    }

    get addressLength() {
        return this.#config.addressLength
    }

    get addressPrefix() {
        return this.#config.addressPrefix
    }

    get addressPrefixLength() {
        return this.addressPrefix.length
    }

    get bech32mHrpLength() {
        return this.#config.bech32mHrpLength
    }

    get bootstrap() {
        return this.#bootstrap
    }

    get channel() {
        return this.#channel
    }

    get dhtBootstrap() {
        if (this.#isOverriden('dhtBootstrap')) return this.#options.dhtBootstrap
        return this.#config.dhtBootstrap
    }

    get disableRateLimit() {
        if (this.#isOverriden('disableRateLimit')) return !!this.#options.disableRateLimit
        return !!this.#config.disableRateLimit
    }

    get enableErrorApplyLogs() {
        if (this.#isOverriden('enableErrorApplyLogs')) return !!this.#options.enableErrorApplyLogs
        return !!this.#config.enableErrorApplyLogs
    }

    get enableInteractiveMode() {
        if (this.#isOverriden('enableInteractiveMode')) return this.#options.enableInteractiveMode !== false
        return !!this.#config.enableInteractiveMode
    }

    get enableRoleRequester() {
        if (this.#isOverriden('enableRoleRequester')) return !!this.#options.enableRoleRequester
        return !!this.#config.enableRoleRequester
    }

    get enableValidatorObserver() {
        if (this.#isOverriden('enableValidatorObserver')) return !!this.#options.enableValidatorObserver
        return !!this.#config.enableValidatorObserver
    }

    get enableTxApplyLogs() {
        if (this.#isOverriden('enableTxApplyLogs')) return !!this.#options.enableTxApplyLogs
        return !!this.#config.enableTxApplyLogs
    }

    get enableWallet() {
        if (this.#isOverriden('enableWallet')) return this.#options.enableWallet !== false
        return !!this.#config.enableWallet
    }

    get isAdminMode() {
        return _.endsWith(this.storesDirectory, '/admin')
    }

    get keyPairDirectoryPath() {
        return `${this.storesFullPath}/db`
    }

    get keyPairPath()  {
        return `${this.keyPairDirectoryPath}/keypair.json`
    }

    get maxRetries() {
        if (this.#isOverriden('maxRetries')) return this.#options.maxRetries
        return this.#config.maxRetries
    }

    get hyperbeeCacheMaxEntries() {
        const value = this.#isOverriden('hyperbeeCacheMaxEntries')
            ? this.#options.hyperbeeCacheMaxEntries
            : this.#config.hyperbeeCacheMaxEntries ?? DEFAULT_HYPERBEE_CACHE_MAX_ENTRIES

        if (!Number.isSafeInteger(value) || value < 1) {
            throw new Error('MainSettlementBus Config: hyperbeeCacheMaxEntries must be a positive safe integer.')
        }

        return value
    }

    get maxValidators() {
        if (this.#isOverriden('maxValidators')) return this.#options.maxValidators
        return this.#config.maxValidators
    }

    get maxPeers() {
        if (this.#isOverriden('maxPeers')) return this.#options.maxPeers
        return this.#config.maxPeers
    }

    get maxParallel() {
        if (this.#isOverriden('maxParallel')) return this.#options.maxParallel
        return this.#config.maxParallel
    }

    get maxServerConnections() {
        if (this.#isOverriden('maxServerConnections')) return this.#options.maxServerConnections
        return this.#config.maxServerConnections
    }

    get maxClientConnections() {
        if (this.#isOverriden('maxClientConnections')) return this.#options.maxClientConnections
        return this.#config.maxClientConnections
    }

    get maxWritersForAdminIndexerConnection() {
        if (this.#isOverriden('maxWritersForAdminIndexerConnection')) return this.#options.maxWritersForAdminIndexerConnection
        return this.#config.maxWritersForAdminIndexerConnection
    }

    get processIntervalMs() {
        if (this.#isOverriden('processIntervalMs')) return this.#options.processIntervalMs
        return this.#config.processIntervalMs
    }

    get transactionPoolSize() {
        if (this.#isOverriden('transactionPoolSize')) return this.#options.transactionPoolSize
        return this.#config.transactionPoolSize
    }

    get networkId() {
        return this.#config.networkId
    }

    get host() {
        if (this.#isOverriden('host')) return this.#options.host
        return this.#config.host
    }

    get port() {
        if (this.#isOverriden('port')) return this.#options.port
        return this.#config.port
    }

    get storesDirectory() {
        const storesDirectory = this.#isOverriden('storesDirectory') ?
            this.#options.storesDirectory : this.#config.storesDirectory

        return _.trimEnd(storesDirectory, '/')
    }

    get storeName() {
        return this.#config.storeName
    }

    get storesFullPath() {
        return `${this.storesDirectory}/${this.storeName}`
    }

    get messageThreshold() {
        return this.#config.messageThreshold
    }

    get messageValidatorRetryDelay() {
        return this.#config.messageValidatorRetryDelay
    }

    get messageValidatorResponseTimeout() {
        return this.#config.messageValidatorResponseTimeout
    }

    get rateLimitCleanupIntervalMs() {
        if (this.#isOverriden('rateLimitCleanupIntervalMs')) return this.#options.rateLimitCleanupIntervalMs
        return this.#config.rateLimitCleanupIntervalMs
    }

    get rateLimitConnectionTimeoutMs() {
        if (this.#isOverriden('rateLimitConnectionTimeoutMs')) return this.#options.rateLimitConnectionTimeoutMs
        return this.#config.rateLimitConnectionTimeoutMs
    }

    get rateLimitMaxTransactionsPerSecond() {
        if (this.#isOverriden('rateLimitMaxTransactionsPerSecond')) return this.#options.rateLimitMaxTransactionsPerSecond
        return this.#config.rateLimitMaxTransactionsPerSecond
    }
    
    get pendingRequestTimeout() {
        return this.#config.pendingRequestTimeout
    }

    get txCommitTimeout() {
        return this.#config.txCommitTimeout
    }

    get txPoolSize() {
        return this.#config.txPoolSize
    }

    get validatorHealthCheckInterval() {
        if (this.#isOverriden('validatorHealthCheckInterval')) return this.#options.validatorHealthCheckInterval
        return this.#config.validatorHealthCheckInterval
    }

    get maxPendingRequestsInPendingRequestsService() {
        return this.#config.maxPendingRequestsInPendingRequestsService
    }

    get debug() {
        return this.#config.debug
    }

    get derivationPath() {
        return this.#config.derivationPath
    }
    
    get pollInterval() {
        if (this.#isOverriden('pollInterval')) return this.#options.pollInterval
        return this.#config.pollInterval
    }

    get adminCacheTTL() {
        if (this.#isOverriden('adminCacheTTL')) return this.#options.adminCacheTTL
        return this.#config.adminCacheTTL
    }
    
    get bootstrapTimeout() {
        if (this.#isOverriden('bootstrapTimeout')) return this.#options.bootstrapTimeout
        return this.#config.bootstrapTimeout
    }

    get writersShortCacheTTL() {
        if (this.#isOverriden('writersShortCacheTTL')) return this.#options.writersShortCacheTTL
        return this.#config.writersShortCacheTTL
    }

    get writersLongCacheTTL() {
        if (this.#isOverriden('writersLongCacheTTL')) return this.#options.writersLongCacheTTL
        return this.#config.writersLongCacheTTL
    }
    
    get validatorConnectionAttemptDelay() {
        if (this.#isOverriden('validatorConnectionAttemptDelay')) return this.#options.validatorConnectionAttemptDelay
        return this.#config.validatorConnectionAttemptDelay
    }

    #isOverriden(prop) {
        return this.#options.hasOwnProperty(prop) && isDefined(this.#options[prop])
    }

    #normalizeBootstrap(bootstrap) {
        if (b4a.isBuffer(bootstrap)) {
            return b4a.from(bootstrap)
        }

        return b4a.from(bootstrap, 'hex')
    }

    #normalizeChannel(channel) {
        return b4a.alloc(32).fill(channel)
    }

    #validateStringOverride(field, value) {
        if (typeof value !== 'string' || value.trim().length === 0) {
            throw new Error(`MainSettlementBus Config: ${field} must be a non-empty string.`);
        }
    }

    #validatePortOverride(port) {
        if (!Number.isInteger(port) || port < 1 || port > 65535) {
            throw new Error('MainSettlementBus Config: port must be an integer in range 1-65535.');
        }
    }

    #validateBootstrapOverride(bootstrap) {
        if (b4a.isBuffer(bootstrap)) {
            if (bootstrap.length !== 32) {
                throw new Error('MainSettlementBus Config: bootstrap must be a 32-byte hex string or Buffer.');
            }
            return;
        }

        if (typeof bootstrap !== 'string' || !/^[0-9a-fA-F]{64}$/.test(bootstrap)) {
            throw new Error('MainSettlementBus Config: bootstrap must be a 32-byte hex string or Buffer.');
        }
    }

    #validateChannelOverride(channel) {
        if (!(typeof channel === 'string' || b4a.isBuffer(channel))) {
            throw new Error('MainSettlementBus Config: channel must be a string or Buffer.');
        }

        const length = b4a.isBuffer(channel) ? channel.length : b4a.from(channel).length

        if (length === 0 || length > 32) {
            throw new Error('MainSettlementBus Config: channel must be 1-32 bytes long.');
        }
    }

    #validateDhtBootstrapOverride(dhtBootstrap) {
        if (!Array.isArray(dhtBootstrap) || dhtBootstrap.length === 0) {
            throw new Error('MainSettlementBus Config: dhtBootstrap must be a non-empty array of strings.');
        }

        for (const entry of dhtBootstrap) {
            if (typeof entry !== 'string' || entry.trim().length === 0) {
                throw new Error('MainSettlementBus Config: dhtBootstrap entries must be non-empty strings.');
            }
        }
    }

    #validate(options, config) {
        if (options === null || typeof options !== 'object') {
            throw new Error('MainSettlementBus Config: options must be an object.');
        }

        if (config === null || typeof config !== 'object') {
            throw new Error('MainSettlementBus Config: config must be an object.');
        }

        if (!options.channel && !config.channel) {
            throw new Error(
                "MainSettlementBus: Channel is required. Application cannot start without channel."
            );
        }

        if (isDefined(options.bootstrap)) {
            this.#validateBootstrapOverride(options.bootstrap);
        }

        if (isDefined(options.channel)) {
            this.#validateChannelOverride(options.channel);
        }

        if (isDefined(options.storesDirectory)) {
            this.#validateStringOverride('storesDirectory', options.storesDirectory);
        }

        if (isDefined(options.host)) {
            this.#validateStringOverride('host', options.host);
        }

        if (isDefined(options.port)) {
            this.#validatePortOverride(options.port);
        }

        if (isDefined(options.dhtBootstrap)) {
            this.#validateDhtBootstrapOverride(options.dhtBootstrap);
        }
    }
}
