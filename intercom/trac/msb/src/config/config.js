import b4a from 'b4a'

export class Config {
    #options
    #config
    #bootstrap
    #channel

    constructor(options = {}, config = {}) {
        this.#validate(options, config)
        this.#options = options
        this.#config = config
        this.#bootstrap = b4a.from(this.#options.bootstrap || this.#config.bootstrap, 'hex')
        // Ensure a 32-byte channel buffer (repeat-fill from string/Buffer if provided)
        this.#channel = b4a.alloc(32).fill(this.#options.channel || this.#config.channel)
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
        return this.#options.storeName === 'admin'
    }

    get keyPairPath()  {
        return `${this.storesFullPath}/db/keypair.json`
    }

    get maxRetries() {
        if (this.#isOverriden('maxRetries')) return this.#options.maxRetries
        return this.#config.maxRetries
    }

    get maxValidators() {
        if (this.#isOverriden('maxValidators')) return this.#options.maxValidators
        return this.#config.maxValidators
    }

    get networkId() {
        return this.#config.networkId
    }

    get storesDirectory() {
        if (this.#isOverriden('storesDirectory')) return this.#options.storesDirectory
        return this.#config.storesDirectory
    }

    get storesFullPath() {
        return `${this.storesDirectory}${this.#options.storeName}`
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

    // Most of these properties are boolean
    #isOverriden(prop) {
        return this.#options.hasOwnProperty(prop)
    }

    #validate(options, config) {
        if (!options.channel && !config.channel) {
            throw new Error(
                "MainSettlementBus: Channel is required. Application cannot start without channel."
            );
        }
    }
}
