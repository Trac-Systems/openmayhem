import Scheduler from "../../../utils/Scheduler.js";
import { Logger } from "../../../utils/logger.js";
import b4a from "b4a";
import { bufferToAddress } from "../../../core/state/utils/address.js";
import tracCryptoApi from "trac-crypto-api";
import { WRITER_BYTE_LENGTH, CONNECTION_STATUS } from "../../../utils/constants.js";

// Internal constants
const VALIDATOR_CANDIDATES_PER_CYCLE = 10;
const MAX_KEY_DECODE_CACHE_SIZE = 10_000;

class ValidatorObserverService {
    #network;
    #state;
    #address;
    #config;
    #scheduler;
    #logger;
    #hasBootstrapped;    // Whether initial connection phase is complete
    #bootstrapStartedAt; // Timestamp to enforce bootstrap timeout
    #adminCache;         // Cached admin entry with TTL
    #writersCache;       // Cached writer list with dynamic TTL
    #isInterrupted;      // Used to stop execution mid-cycle
    #keyDecodeCache;

    /**
     * @param {Object} network - The network layer instance (e.g., Hyperswarm wrapper).
     * @param {Object} state - The state manager instance interacting with Autobase.
     * @param {string} address - The physical address of the current node.
     * @param {Object} config - The application configuration object.
     */
    constructor(network, state, address, config) {
        this.#network = network;
        this.#state = state;
        this.#address = address;
        this.#config = config;
        this.#logger = new Logger(config);
        this.#hasBootstrapped = false;
        this.#bootstrapStartedAt = 0;
        this.#adminCache = { entry: null, lastUpdated: 0 };
        this.#writersCache = { list: [], lastUpdated: 0 };
        this.#isInterrupted = false;
        this.#keyDecodeCache = new Map();
    }

    /**
     * Returns configurable poll interval.
     * @returns {number} The poll interval in milliseconds.
     */
    #getPollInterval() {
        return this.#config.pollInterval;
    }
    
    /**
     * Selects a subset of valid validator candidates to connect to.
     *
     * Rules:
     * - Assumes writers list is already pre-filtered and deduplicated from scan phase.
     * - Excludes self.
     * - Excludes already connected or pending nodes.
     * - Applies admin connection restriction based on the threshold of ALIVE writers.
     * - Randomizes selection (Fisher-Yates) to avoid bias.
     * @param {Array<Object>} writers - The deduplicated list of active writers.
     * @param {Object} adminEntry - The current admin node entry.
     * @returns {{candidates: Array<Object>, slots: number}} The selected candidates and the number of available connection slots.
     */
    #selectCandidates(writers, adminEntry) {
        const manager = this.#network.validatorConnectionManager;
        const adminThreshold = this.#config.maxWritersForAdminIndexerConnection;
        
        // Evaluates the admin blockage policy based on fully connected/alive writers
        const aliveWritersCount = this.#countAliveWriters(writers, adminEntry);
        const adminBlocked = adminEntry && aliveWritersCount >= adminThreshold;
        
        // Target calculation: ensures the observer deducts its own slot from the network size 
        // only if it belongs to the valid writers list.
        const isSelfInWriters = writers.some(w => w.address === this.#address);
        const networkSize = isSelfInWriters ? writers.length - 1 : writers.length;
        const target = Math.min(networkSize, this.#config.maxValidators);
        
        const isSelfAdmin = this.#address === adminEntry?.address;
        const totalConnected = manager.connectedValidators().length;

        const available = writers.filter((w) => {
            if (w.address === this.#address) return false;

            const isConnected = manager.connected(w.publicKey);
            
            // Checks if we are ALREADY trying to connect to this specific node
            const isPending = this.#network.isConnectionPending(w.publicKeyHex);

            // Skips nodes we are already interacting with
            if (isConnected || isPending) return false;
            
            // The admin is actively filtered out from the pool if the threshold policy is triggered
            if (adminBlocked && !isSelfAdmin && w.address === adminEntry?.address) return false;

            return true;
        });

        const result = [...available];

        // Fisher-Yates shuffle
        for (let i = 0; i < result.length; i++) {
            const j = i + Math.floor(Math.random() * (result.length - i));
            [result[i], result[j]] = [result[j], result[i]];
        }

        // Open slots based ONLY on who is ALREADY connected,
        // Ignoring pending/non-active nodes to avoid blocking the queue.
        const slots = Math.max(0, target - totalConnected);
        const limit = Math.min(slots, VALIDATOR_CANDIDATES_PER_CYCLE, result.length);
        
        return {
            candidates: result.slice(0, limit),
            slots: slots,
        }
    }
   
    /**
     * Enforces admin-specific connection rules.
     *
     * If the count of fully connected writers exceeds the threshold:
     * - The admin node is excluded from the validator pool (handled in selection phase).
     * - Non-admin nodes actively disconnect from the admin validator (if currently connected).
     *
     * NOTE:
     * The admin is not forcibly disconnected from all roles.
     * It can still maintain connections for tx submission and observation.
     * @param {Array<Object>} writers - The deduplicated list of active writers.
     * @param {Object} adminEntry - The current admin node entry.
     */
    #enforceAdminPolicy(writers, adminEntry) {
        // Evaluates against alive network peers rather than dormant database entries
        const aliveWritersCount = this.#countAliveWriters(writers, adminEntry);
        
        if (aliveWritersCount < this.#config.maxWritersForAdminIndexerConnection) return;

        this.#logger.debug("Validator threshold reached → enforcing admin policy");

        const isAdmin = adminEntry && this.#address === adminEntry.address;
        if (isAdmin) return;

        // Non-admin nodes: ensure the admin is not kept as a validator connection
        const adminWriter = writers.find((w) => w.address === adminEntry?.address);
        if (adminWriter?.publicKey) {
            this.#network.validatorConnectionManager.remove(adminWriter.publicKey);
        }
    }

    /**
     * Removes active connections to writers that are no longer valid (e.g., removed/soft-deleted).
     * @param {Uint8Array} publicKey - The public key of the stale connection to be removed.
     */
    #enforceNoStaleConnections(publicKey) {
        const manager = this.#network.validatorConnectionManager;

        if (manager.connected(publicKey)) {
            manager.remove(publicKey);
            this.#logger.debug(`Removed stale validator connection: ${b4a.toString(publicKey, "hex")}`);
        }
    }
    
    /**
     * Determines whether the observer should run.
     * Controlled by a config flag + runtime interruption.
     * @returns {boolean} True if the observer is enabled and not interrupted.
     */
    #shouldRun() {
        return this.#config.enableValidatorObserver && !this.#isInterrupted;
    }
    
    /**
     * Counts the number of physically active and connected writers in the network.
     * This method bridges the gap between the Autobase state (which may include 
     * offline/dormant writers due to its append-only/soft-delete nature) and the 
     * actual physical network layer. It ensures the admin threshold policy is 
     * evaluated against truly active nodes rather than historical state.
     *
     * @param {Array<Object>} writers - The deduplicated list of writers from the Autobase.
     * @param {Object} adminEntry - The current admin node entry.
     * @returns {number} The count of alive, fully connected writers (including self).
     */
    #countAliveWriters(writers, adminEntry) {
        let aliveCount = 0;
        const manager = this.#network.validatorConnectionManager;

        for (const w of writers) {
            // Ignores the admin in the threshold calculation
            if (w.address === adminEntry?.address) continue;

            // Always counts the current node as alive if it's in the valid writers list
            if (w.address === this.#address) {
                aliveCount++;
                continue;
            }

            // Counts only fully connected nodes (ignores pending or offline peers)
            if (manager.connected(w.publicKey)) {
                aliveCount++;
            }
        }

        return aliveCount;
    }

    /**
     * Starts the validator observer loop.
     * Initializes the scheduler and bootstrap state.
     */
    async start() {
        if (!this.#shouldRun()) {
            this.#logger.info('ValidatorObserverService can not start. Disabled by configuration.');
            return;
        }
        
        if (this.#scheduler && this.#scheduler.isRunning) {
            this.#logger.info('ValidatorObserverService is already started');
            return;
        }

        const interval = this.#getPollInterval();
        this.#scheduler = new Scheduler((next) => this.#worker(next), interval);
        this.#bootstrapStartedAt = Date.now();
        this.#hasBootstrapped = false;
        this.#isInterrupted = false;
        this.#scheduler.start();
        this.#logger.debug("ValidatorObserverService started");
    }

    /**
     * Stops the observer gracefully.
     * Prevents new work and waits for the current cycle to finish if requested.
     * @param {boolean} [waitForCurrent=true] - Whether to wait for the current worker cycle to finish.
     */
    async stopValidatorObserver(waitForCurrent = true) {
        if (!this.#scheduler) return;

        this.#isInterrupted = true;

        await this.#scheduler.stop(waitForCurrent);
        this.#scheduler = null;
        this.#logger.debug("ValidatorObserverService stopped");
    }

    /**
     * Main worker loop (executed periodically).
     *
     * Flow:
     * 1. Fetch admin + writers (cached).
     * 2. Enforce admin connection policy.
     * 3. Skip if the connection pool is full.
     * 4. Select candidates.
     * 5. Attempt connections (with pacing).
     * @param {Function} next - Callback to schedule the next execution.
     */
    async #worker(next) {
        const interval = this.#getPollInterval();
        if (!this.#shouldRun()) return next(interval);

        try {
            const adminEntry = await this.#getAdminEntryCached();
            const writers = await this.#getWritersCached(adminEntry);
            
            if (writers.length === 0) return next(interval);

            this.#enforceAdminPolicy(writers, adminEntry);

            const manager = this.#network.validatorConnectionManager;
            // CEILING: do not exceed the max connections limit
            if (manager.maxConnectionsReached()) return next(interval); 
            
            const { candidates, slots} = this.#selectCandidates(writers, adminEntry);
            if (candidates.length > 0) {
                await this.#processCandidates(candidates, slots);
            }

        } catch (err) {
            this.#logger.error(`ValidatorObserver worker error: ${err.message}`);
        }

        next(interval);
    }

    /**
     * Returns the cached admin entry with TTL.
     * @returns {Promise<Object>} The admin entry object.
     */
    async #getAdminEntryCached() {
        const now = Date.now();
        const ttl = this.#config.adminCacheTTL;
        if (this.#adminCache.entry && now - this.#adminCache.lastUpdated < ttl) return this.#adminCache.entry;

        const entry = await this.#state.getAdminEntry();
        this.#adminCache = { entry, lastUpdated: now };
        return entry;
    }

    /**
     * Returns the cached writer list with dynamic TTL:
     * - Short TTL during bootstrap (aggressive discovery).
     * - Long TTL after bootstrap (stability).
     *
     * Bootstrap completes when:
     * - Connection pool is full OR
     * - Timeout is reached.
     * @param {Object} adminEntry - The current admin node entry.
     * @returns {Promise<Array<Object>>} The deduplicated list of writers.
     */
    async #getWritersCached(adminEntry) {
        const now = Date.now();
        const cachedList = this.#writersCache.list;
        const lastUpdated = this.#writersCache.lastUpdated;
        const manager = this.#network.validatorConnectionManager;

        if (!this.#hasBootstrapped && manager.maxConnectionsReached()) {
            this.#hasBootstrapped = true;
            this.#logger.debug("Bootstrap complete (pool full)");
        }

        if (!this.#hasBootstrapped && now - this.#bootstrapStartedAt > this.#config.bootstrapTimeout) {
            this.#hasBootstrapped = true;
            this.#logger.debug("Bootstrap complete (timeout)");
        }
        
        const short = this.#config.writersShortCacheTTL;
        const long = this.#config.writersLongCacheTTL;
        
        const dynamicTTL = this.#hasBootstrapped ? long : short;
        if (cachedList.length > 0 && now - lastUpdated < dynamicTTL) return cachedList;
            
        const list = await this.#scanAutobaseWriters(adminEntry);
        this.#writersCache = { list, lastUpdated: now };

        return list;
    }

    /**
     * Sequentially processes selected candidates.
     * Applies delay between connection attempts to avoid network bursts.
     * @param {Array<Object>} candidates - The list of selected validator candidates.
     * @param {number} slots - The number of available connection slots.
     */
    async #processCandidates(candidates, slots) {
        const delay = this.#config.validatorConnectionAttemptDelay;

        let attempts = 0;

        for (const candidate of candidates) {
            if (!this.#shouldRun()) break;
            if (attempts >= slots) break;

            const isConnected = this.#network.validatorConnectionManager.connected(candidate.publicKey);
            const isPending = this.#network.isConnectionPending(candidate.publicKeyHex);

            if (isConnected || isPending) continue;

            const result = await this.#tryConnect(candidate);
            if (!result || result === CONNECTION_STATUS.IGNORED) continue;
            this.#logger.debug(`Validator connection to ${candidate.publicKeyHex} resulted in: ${result}`);

            attempts++;

            if (attempts < slots) {
                await new Promise((resolve) => setTimeout(resolve, delay));
            }
        }
    }

    /**
     * Scans Autobase writer entries and builds the active validator list.
     *
     * Rules:
     * - Skips invalid/malformed entries.
     * - Skips non-admin indexers.
     * - Deduplicates nodes by mapping their latest state to a unique address.
     * - Removes stale connections for nodes flagged with 'isRemoved'.
     * - Includes only valid and active writer nodes.
     * @param {Object} adminEntry - The current admin node entry.
     * @returns {Promise<Array<Object>>} A promise resolving to the deduplicated array of active writers.
     */
    async #scanAutobaseWriters(adminEntry) {
        // Prevents memory leak in case of a very long-running node
        if (this.#keyDecodeCache.size > MAX_KEY_DECODE_CACHE_SIZE) {
            this.#keyDecodeCache.clear();
        }

        // Map is used for deduplication, ensuring that each node physical address
        // occupies only a single slot despite the append-only nature of Autobase
        const activeMap = new Map();

        try {
            for await (const { key, value } of this.#state.base.system.list()) {
                if (!this.#shouldRun()) break;

                if (!key || b4a.byteLength(key) !== WRITER_BYTE_LENGTH) continue;
                if (!value) continue;

                const writerKeyHex = b4a.toString(key, "hex");
                let decodedIdentity = this.#keyDecodeCache.get(writerKeyHex);

                if (decodedIdentity === undefined) {
                    const addressBuffer = await this.#state.getRegisteredWriterKey(writerKeyHex);

                    if (!addressBuffer || b4a.byteLength(addressBuffer) !== this.#config.addressLength) {
                        this.#keyDecodeCache.set(writerKeyHex, null);
                        continue;
                    }

                    const addr = bufferToAddress(addressBuffer, this.#config.addressPrefix);
                    const publicKey = tracCryptoApi.address.decode(addr);

                    if (!publicKey) {
                        this.#keyDecodeCache.set(writerKeyHex, null);
                        continue;
                    }

                    const publicKeyHex = b4a.toString(publicKey, "hex");
                    decodedIdentity = { addr, publicKey, publicKeyHex };
                    this.#keyDecodeCache.set(writerKeyHex, decodedIdentity);
                }

                if (decodedIdentity === null) continue;

                const { addr, publicKey, publicKeyHex } = decodedIdentity;

                if (value.isIndexer && addr !== adminEntry?.address) continue;

                // Handles soft-delete: drops stale connections and removes the node from the pool
                if (value.isRemoved) {
                    this.#enforceNoStaleConnections(publicKey);
                    activeMap.delete(addr);
                    continue;
                }

                // Registers or overwrites to maintain a single source of truth per machine
                activeMap.set(addr, { key, address: addr, publicKey, publicKeyHex });
            }
        } catch (err) {
            this.#logger.error(`Autobase writer scan error: ${err.message}`);
        }

        return Array.from(activeMap.values());
    }

    /**
     * Attempts to establish a connection with a validator candidate.
     *
     * Assumes the candidate was pre-filtered and validated during scan/selection.
     *
     * Behavior:
     * - Delegates connection attempt to the network layer.
     * @param {Object} candidate - The target validator candidate object.
     * @returns {Promise<string>} The connection status string returned by the network layer.
     */
    async #tryConnect(candidate) {
        try {
            return await this.#network.tryConnect(candidate.publicKeyHex, "validator");
        } catch (err) {
            this.#logger.error(`Validator connection attempt failed: ${err.message}`);
        }
    }
}

export default ValidatorObserverService;