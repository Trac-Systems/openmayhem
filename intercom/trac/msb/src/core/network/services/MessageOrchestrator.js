import { generateUUID, publicKeyToAddress } from '../../../utils/helpers.js';
import { operationToPayload } from '../../../utils/applyOperations.js';
import { networkMessageFactory } from "../../../messages/network/v1/networkMessageFactory.js";
import { NETWORK_CAPABILITIES, ResultCode } from "../../../utils/constants.js";
import {
    unsafeEncodeApplyOperation
} from "../../../utils/protobuf/operationHelpers.js";
import { normalizeMessageByOperationType } from "../../../utils/normalizers.js";
import { resultToValidatorAction, SENDER_ACTION } from "../protocols/connectionPolicies.js";
import { ConnectionManagerError } from './ConnectionManager.js';
/**
 * MessageOrchestrator coordinates message submission, retry, and validator management.
 * It works with ConnectionManager and ledger state to ensure reliable message delivery.
 */
class MessageOrchestrator {
    #config;
    #wallet;
    #idempotentSuccessCodes = new Set([
        ResultCode.TX_ALREADY_EXISTS,
        ResultCode.OPERATION_ALREADY_COMPLETED,
    ]);
    /**
     * Attempts to send a message to validators with retries and state checks.
     * @param {ConnectionManager} connectionManager - The connection manager instance
     * @param {object} state - The state to look for the message outcome
     * @param {Config} config - Configuration options:
     */
    constructor(connectionManager, state, config) {
        this.connectionManager = connectionManager;
        this.state = state;
        this.#config = config;
        this.#wallet = null;
    }

    setWallet(wallet) {
        this.#wallet = wallet;
    }

    /**
     * Picks a validator for an outgoing message while avoiding requester self-validation.
     *
     * ValidatorObserverService already prevents connecting to the local node itself.
     * This method handles a different case: for a given message, we avoid selecting
     * a validator whose address equals `message.address` (requester), because
     * validator-side checks reject that flow.
     *
     * @param {object} [message] Outgoing operation payload.
     * @param {string} [message.address] Requester address (bech32m).
     * @returns {string|null} Selected validator public key hex, or null when unavailable.
     */
    #pickValidatorForMessage(message) {
        const requesterAddress = message?.address;
        if (!requesterAddress || typeof this.connectionManager.connectedValidators !== 'function') {
            return this.connectionManager.pickRandomConnectedValidator();
        }

        const connected = this.connectionManager.connectedValidators();
        if (!Array.isArray(connected) || connected.length === 0) {
            return null;
        }

        const eligible = connected.filter((publicKey) => {
            return publicKeyToAddress(publicKey, this.#config) !== requesterAddress;
        });

        const pool = eligible.length > 0 ? eligible : connected;
        if (typeof this.connectionManager.pickRandomValidator === 'function') {
            return this.connectionManager.pickRandomValidator(pool);
        }

        const index = Math.floor(Math.random() * pool.length);
        return pool[index] ?? null;
    }

    /**
     * Sends a message to a single randomly selected connected validator.
     * @param {object} message - The message object to be sent
     * @param retries - The current retry count for this message
     * @returns {Promise<boolean>} - true if successful, false otherwise
     */
    async send(message, retries = 0) {
        if (retries > this.#config.maxRetries) {
            console.warn(`MessageOrchestrator: Max retries reached for message ${JSON.stringify(message)}. Aborting send.`);
            return false;
        }

        const validatorPublicKey = this.#pickValidatorForMessage(message);
        if (!validatorPublicKey) return false;
        console.log("Sending message to validator:", publicKeyToAddress(validatorPublicKey, this.#config));

        /* NOTE: Since the retry logic for Legacy is handled here, and is very unique to the protocol,
        * it was decided to not change MessageOrchestrator send method in the refactor to make protocols transparent.
        * As the Legacy protocol is going to be deprecated soon, it was decided to keep the retry logic 
        * here instead of abstracting it in the protocol implementation. 
        * If we were to abstract it, we would need to add protocol-specific logic in the ProtocolSession
        * or ProtocolInterface, which would make them less clean and more coupled with the specifics of the protocols.
        * The parts to be refactored in the future are marked with TODO comments.
        */

        // TODO: After Legacy is deprecated, we don't need to check preferred protocol here.
        const validatorConnection = this.connectionManager.getConnection(validatorPublicKey);
        const preferredProtocol = validatorConnection.protocolSession.preferredProtocol;
        let success = false;
        if (preferredProtocol === validatorConnection.protocolSession.supportedProtocols.LEGACY) {

            try {
                success = await this.#attemptSendMessageForLegacy(validatorPublicKey, message);
            } catch {
                success = await this.send(message, retries + 1);
            }
            if (!success) {
                // Remove validator and retry
                this.connectionManager.remove(validatorPublicKey);
                success = await this.send(message, retries + 1);
            }
        } else if (preferredProtocol === validatorConnection.protocolSession.supportedProtocols.V1) {
            // TODO: This is probably better placed inside the V1 protocol definition.
            // Both protocols should receive a 'canonical' message and solve the encodings internally
            // Refactor 
            const normalizedMessage = normalizeMessageByOperationType(message, this.#config)
            const encodedTransaction = unsafeEncodeApplyOperation(normalizedMessage)
            const v1Message = await networkMessageFactory(this.#wallet, this.#config)
                .buildBroadcastTransactionRequest(
                    generateUUID(),
                    encodedTransaction,
                    NETWORK_CAPABILITIES
                );

            await this.connectionManager.sendSingleMessage(v1Message, validatorPublicKey)
                .then(
                    async (resultCode) => {
                        if (await this.#isIdempotentSuccess(resultCode, message)) {
                            success = true;
                            return;
                        }

                        // TODO: When we will deprecate the legacy protocol, we should refactor this scope, to propagate domain-error with result code.
                        const action = resultToValidatorAction(resultCode);
                        switch (action) {
                            case SENDER_ACTION.SUCCESS:
                                success = true;
                                //TODO: Create a function for action below, and replace it also in legacy flow.
                                this.incrementSentCount(validatorPublicKey);
                                if (this.shouldRemove(validatorPublicKey)) {
                                    this.connectionManager.remove(validatorPublicKey);
                                }
                                break;
                            case SENDER_ACTION.ROTATE:
                                this.connectionManager.remove(validatorPublicKey);
                                break;
                            case SENDER_ACTION.NO_ROTATE:
                                // ignore
                                break;
                            default:
                                this.connectionManager.remove(validatorPublicKey);
                                console.warn(
                                    `MessageOrchestrator: Unrecognized action from connectionPolicies: ${action}.
                                     ResultCode was: ${resultCode}. Removing validator ${publicKeyToAddress(validatorPublicKey, this.#config)}`
                                );
                                break;
                        }
                    }
                )
                .catch(
                    async (err) => {
                        if (err instanceof ConnectionManagerError) {
                            success = await this.send(message, retries + 1);
                            console.warn(`MessageOrchestrator: Connection Error: ${err.message}`);
                        } else {
                            this.connectionManager.remove(validatorPublicKey);
                            success = await this.send(message, retries + 1);
                        }
                    }
                )

        }
        return success;
    }

    /**
     * Determines whether a non-OK result code should be treated as a success due to idempotency.
     *
     * This handles the retry scenario where a requester did not receive or accept the response
     * from the first validator that committed the transaction. On retry, a second validator may
     * return TX_ALREADY_EXISTS or OPERATION_ALREADY_COMPLETED because the tx was already
     * processed. Those codes are not errors — they confirm the operation succeeded.
     *
     * To safely treat these as success, we check whether the local unsigned state for the tx
     * has been committed. This ensures we only acknowledge idempotent success when the state
     * change is actually observable locally, not just because the result code matched.
     */
    async #isIdempotentSuccess(resultCode, message) {
        if (!this.#idempotentSuccessCodes.has(resultCode)) return false;

        const txHash = this.#extractTxHash(message);
        if (!txHash) return false;

        // A short wait covers the race where a first validator committed the tx
        // and a retried validator only observes it as "already exists/completed".
        const timeout = this.#config.messageValidatorResponseTimeout ?? 2000;
        return await this.waitForUnsignedState(txHash, timeout);
    }

    async waitForUnsignedState(txHash, timeout) {
        return this.state.waitForUnsigned(txHash, timeout);
    }

    #extractTxHash(message) {
        if (!message || !Number.isInteger(message.type)) return null;

        const payloadKey = operationToPayload(message.type);
        const txHash = message?.[payloadKey]?.tx;
        return typeof txHash === 'string' && txHash.length > 0 ? txHash : null;
    }

    // TODO: Delete this function after legacy protocol is deprecated
    async #attemptSendMessageForLegacy(validatorPublicKey, message) {
        const deductedTxType = operationToPayload(message.type);
        await this.connectionManager.sendSingleMessage(message, validatorPublicKey);
        const appeared = await this.state.waitForUnsigned(
            message[deductedTxType].tx,
            this.#config.messageValidatorResponseTimeout
        );
        if (appeared) {
            this.incrementSentCount(validatorPublicKey);
            if (this.shouldRemove(validatorPublicKey)) {
                this.connectionManager.remove(validatorPublicKey);
            }
            return true;
        }
        return false;
    }

    incrementSentCount(validatorPubKey) {
        this.connectionManager.incrementSentCount(validatorPubKey);
    }

    shouldRemove(validatorPubKey) {
        return this.connectionManager.getSentCount(validatorPubKey) >= this.#config.messageThreshold;
    }
}

export default MessageOrchestrator;
