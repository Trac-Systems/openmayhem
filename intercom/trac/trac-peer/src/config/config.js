import b4a from "b4a";
import path from "path";

export class Config {
    #options;

    constructor(options = {}, defaults = {}) {
        this.#validate(options, defaults);
        this.#options = options;

        const storesDirectoryRaw = this.#select("storesDirectory", options, defaults);
        if (typeof storesDirectoryRaw !== "string" || storesDirectoryRaw.length === 0) {
            throw new Error("Peer: storesDirectory is required.");
        }
        this.storesDirectory = storesDirectoryRaw.endsWith("/")
            ? storesDirectoryRaw
            : `${storesDirectoryRaw}/`;

        const storeNameRaw = this.#select("storeName", options, defaults);
        if (typeof storeNameRaw !== "string" || storeNameRaw.length === 0) {
            throw new Error("Peer: storeName is required.");
        }
        this.storeName = storeNameRaw.replace(/^\/+|\/+$/g, "");
        if (this.storeName.length === 0) {
            throw new Error("Peer: storeName is required.");
        }

        // Keep path assembly stable (avoid double slashes like "stores//peer/db/...").
        this.fullStoresDirectory = path.join(this.storesDirectory, this.storeName);
        this.keyPairPath = path.join(this.fullStoresDirectory, "db", "keypair.json");

        const txPoolMaxSize = this.#select("txPoolMaxSize", options, defaults);
        if (!Number.isSafeInteger(txPoolMaxSize)) {
            throw new Error("Peer: txPoolMaxSize must be a safe integer.");
        }
        this.txPoolMaxSize = txPoolMaxSize;

        const maxTxDelay = this.#select("maxTxDelay", options, defaults);
        if (!Number.isSafeInteger(maxTxDelay)) {
            throw new Error("Peer: maxTxDelay must be a safe integer.");
        }
        this.maxTxDelay = maxTxDelay;

        const maxMsbSignedLength = this.#select("maxMsbSignedLength", options, defaults);
        if (!Number.isSafeInteger(maxMsbSignedLength)) {
            throw new Error("Peer: maxMsbSignedLength must be a safe integer.");
        }
        this.maxMsbSignedLength = maxMsbSignedLength;

        const maxMsbApplyOperationBytes = this.#select("maxMsbApplyOperationBytes", options, defaults);
        if (!Number.isSafeInteger(maxMsbApplyOperationBytes)) {
            throw new Error("Peer: maxMsbApplyOperationBytes must be a safe integer.");
        }
        this.maxMsbApplyOperationBytes = maxMsbApplyOperationBytes;

        // Interactive mode defaults to enabled unless explicitly disabled.
        const enableInteractiveModeRaw = this.#select("enableInteractiveMode", options, defaults);
        if (enableInteractiveModeRaw === undefined) {
            throw new Error("Peer: enableInteractiveMode must be set.");
        }
        this.enableInteractiveMode = enableInteractiveModeRaw !== false;

        const enableBackgroundTasksRaw = this.#select("enableBackgroundTasks", options, defaults);
        if (enableBackgroundTasksRaw === undefined) {
            throw new Error("Peer: enableBackgroundTasks must be set.");
        }
        this.enableBackgroundTasks = enableBackgroundTasksRaw !== false;

        const enableUpdaterRaw = this.#select("enableUpdater", options, defaults);
        if (enableUpdaterRaw === undefined) {
            throw new Error("Peer: enableUpdater must be set.");
        }
        this.enableUpdater = enableUpdaterRaw !== false;

        const replicateRaw = this.#select("replicate", options, defaults);
        if (replicateRaw === undefined) {
            throw new Error("Peer: replicate must be set.");
        }
        this.replicate = replicateRaw !== false;

        const channelRaw = this.#select("channel", options, defaults);
        if (channelRaw === null || channelRaw === undefined || channelRaw === "") {
            throw new Error("Peer: channel is required.");
        }
        this.channel = b4a.alloc(32).fill(channelRaw);

        const dhtBootstrap = this.#select("dhtBootstrap", options, defaults);
        if (dhtBootstrap === undefined) {
            throw new Error("Peer: dhtBootstrap must be set.");
        }
        this.dhtBootstrap = dhtBootstrap;

        const enableTxlogsRaw = this.#select("enableTxlogs", options, defaults);
        if (enableTxlogsRaw === undefined) {
            throw new Error("Peer: enableTxlogs must be set.");
        }
        this.enableTxlogs = enableTxlogsRaw;

        // Protocol API exposure flags.
        const apiTxExposedRaw = this.#select("apiTxExposed", options, defaults);
        if (apiTxExposedRaw === undefined) {
            throw new Error("Peer: apiTxExposed must be set.");
        }
        this.apiTxExposed = apiTxExposedRaw === true;

        const apiMsgExposedRaw = this.#select("apiMsgExposed", options, defaults);
        if (apiMsgExposedRaw === undefined) {
            throw new Error("Peer: apiMsgExposed must be set.");
        }
        this.apiMsgExposed = apiMsgExposedRaw === true;

        const bootstrapRaw = this.#select("bootstrap", options, defaults);
        if (bootstrapRaw === undefined) {
            throw new Error("Peer: bootstrap must be set (null allowed).");
        }
        if (bootstrapRaw === null) {
            this.bootstrap = null;
        } else if (b4a.isBuffer(bootstrapRaw)) {
            if (bootstrapRaw.length !== 32) throw new Error("Peer: bootstrap must be 32-byte hex.");
            this.bootstrap = bootstrapRaw;
        } else if (typeof bootstrapRaw === "string") {
            if (!/^[0-9a-fA-F]{64}$/.test(bootstrapRaw)) {
                throw new Error("Peer: bootstrap must be 32-byte hex.");
            }
            this.bootstrap = b4a.from(bootstrapRaw, "hex");
        } else {
            throw new Error("Peer: bootstrap must be 32-byte hex.");
        }
    }

    #isOverriden(prop) {
        return Object.prototype.hasOwnProperty.call(this.#options, prop);
    }

    #select(prop, options, defaults) {
        if (this.#isOverriden(prop)) return options[prop];
        if (Object.prototype.hasOwnProperty.call(defaults, prop)) return defaults[prop];
        return undefined;
    }

    #validate(options, defaults) {
        if (options === null || typeof options !== "object") {
            throw new Error("Peer: options must be an object.");
        }
        if (defaults === null || typeof defaults !== "object") {
            throw new Error("Peer: defaults must be an object.");
        }
    }
}
