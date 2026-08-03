import b4a from 'b4a';
import {V1ProtocolError} from "../protocols/v1/V1ProtocolError.js";
import {ResultCode} from "../../../utils/constants.js";
import {publicKeyToAddress} from "../../../utils/helpers.js";

class TransactionRateLimiterService {
    #lastCleanup;
    #connectionsStatistics;
    #swarm;
    #config;

    constructor(swarm, config) {
        this.#lastCleanup = Date.now();
        this.#connectionsStatistics = new Map();
        this.#swarm = swarm
        this.#config = config
    }

    /*
        Checks if the peer has exceeded the rate limit for the current 1-second window.
        A peer is considered to have exceeded the rate limit if:
        - The request belongs to the same 1-second window as previous requests (tracked per peer)
        - The number of transactions already seen in this window is >= rateLimitMaxTransactionsPerSecond

        Important:
        - This method assumes the caller increments transactionCount AFTER calling this method.
          (So exactly rateLimitMaxTransactionsPerSecond are allowed; the next one is blocked.)
    */
    #hasExceededRateLimit(peer, currentTime) {
        const peerData = this.#connectionsStatistics.get(peer);
        const currentSecond = Math.floor((currentTime - peerData.sessionStartTime) / 1000);
        const lastResetSecond = Math.floor((peerData.lastCounterReset - peerData.sessionStartTime) / 1000);

        if (currentSecond > lastResetSecond) {
            peerData.transactionCount = 0;
            peerData.lastCounterReset = currentTime;
            this.#connectionsStatistics.set(peer, peerData);
        }
        
        return peerData.transactionCount >= this.#config.rateLimitMaxTransactionsPerSecond;
    }

    /*
        Handles rate limiting for a peer connection (legacy protocol).
        If the peer has exceeded the rate limit, it disconnects the peer and returns true.
        Otherwise, it updates the connection info with the current timestamp and returns false.
    */
    legacyHandleRateLimit(connection) {
        const peer = b4a.toString(connection.remotePublicKey, 'hex');
        const currentTime = Date.now();

        this.#cleanUpOldConnections(currentTime);
        this.#initializePeerConnectionInfoEntry(peer, currentTime);

        if (this.#hasExceededRateLimit(peer, currentTime)) {
            console.warn(`Rate limit exceeded for peer ${peer}. Disconnecting...`);
            this.#swarm.leavePeer(connection.remotePublicKey);
            connection.end();
            return true;
        }

        this.#updatePeerConnectionInfo(peer, currentTime);
        return false;
    }

    /*
        Handles rate limiting for a peer connection (v1 protocol).
        If the peer has exceeded the rate limit, it throws V1ProtocolError with RATE_LIMITED result code.
        Otherwise, it updates the connection info with the current timestamp.
    */
    v1HandleRateLimit(connection) {
        const peer = b4a.toString(connection.remotePublicKey, 'hex');
        const currentTime = Date.now();

        this.#cleanUpOldConnections(currentTime);
        this.#initializePeerConnectionInfoEntry(peer, currentTime);

        if (this.#hasExceededRateLimit(peer, currentTime)) {
            throw new V1ProtocolError(
                ResultCode.RATE_LIMITED,
                `Rate limit exceeded for peer ${publicKeyToAddress(connection.remotePublicKey, this.#config)}`
            );
        }
        this.#updatePeerConnectionInfo(peer, currentTime);
    }

    #shouldCleanupConnections(currentTime) {
        return currentTime - this.#lastCleanup >= this.#config.rateLimitCleanupIntervalMs;
    }

    /**
        Cleans up per-peer statistics that have been inactive for more than rateLimitCleanupIntervalMs.
        Runs at most once every rateLimitCleanupIntervalMs.
    */
    #cleanUpOldConnections(currentTime) {
        if (!this.#shouldCleanupConnections(currentTime)) {
            return;
        }

        for (const [peer] of this.#connectionsStatistics.entries()) {
            if (this.#isConnectionExpired(peer, currentTime)) {
                this.#connectionsStatistics.delete(peer);
            }
        }

        this.#lastCleanup = currentTime;
    }

    /*
        Initializes the connection statistics for a peer.
        Stored as a HashMap with the following structure:
            peerPublicKeyHex: {
                sessionStartTime: timestamp,  // When we first saw this peer (start of local tracking session)
                lastActivityTime: timestamp,  // Timestamp of peer's most recent activity
                lastCounterReset: timestamp,  // Timestamp when the per-second counter was last reset
                transactionCount: number      // Transactions seen in the current 1-second window
            }
    */
    #initializePeerConnectionInfoEntry(peer, timestamp) {
        if (!this.#connectionsStatistics.has(peer)) {
            this.#connectionsStatistics.set(peer, {
                sessionStartTime: timestamp,
                lastActivityTime: timestamp,
                lastCounterReset: timestamp,
                transactionCount: 0
            });
        }
    }
    
    /*
        When external peer sends a transaction, this method updates the connection info.
        It updates the last activity time and increments the transaction count.
    */
    #updatePeerConnectionInfo(peer, timestamp) {
        const peerData = this.#connectionsStatistics.get(peer);
        peerData.lastActivityTime = timestamp;
        peerData.transactionCount += 1;
        this.#connectionsStatistics.set(peer, peerData);
    }

    /*
        Checks if the stored statistics for a peer have expired due to inactivity.
        Note: this is NOT a network-level connection timeout; it's only used to evict old Map entries.
    */
    #isConnectionExpired(peer, currentTime) {
        const peerData = this.#connectionsStatistics.get(peer);
        return currentTime - peerData.lastActivityTime >= this.#config.rateLimitConnectionTimeoutMs;
    }
}

export default TransactionRateLimiterService;
