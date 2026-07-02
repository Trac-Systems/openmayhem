import ReadyResource from 'ready-resource';
import Autobase from 'autobase';
import Hyperbee from 'hyperbee';
import b4a from 'b4a';
import {
    ACK_INTERVAL,
    ADMIN_INITIAL_BALANCE,
    EntryType,
    OperationType,
    AUTOBASE_VALUE_ENCODING,
    HYPERBEE_KEY_ENCODING,
    HYPERBEE_VALUE_ENCODING,
    BATCH_SIZE,
    ADMIN_INITIAL_STAKED_BALANCE,
    MAX_WRITERS_FOR_ADMIN_INDEXER_CONNECTION,
    TRAC_NAMESPACE,
    CustomEventType
} from '../../utils/constants.js';
import { isHexString, sleep, isTransactionRecordPut } from '../../utils/helpers.js';
import PeerWallet from 'trac-wallet';
import Check from '../../utils/check.js';
import { safeDecodeApplyOperation } from '../../utils/protobuf/operationHelpers.js';
import { createMessage, ZERO_WK } from '../../utils/buffer.js';
import addressUtils from './utils/address.js';
import adminEntryUtils from './utils/adminEntry.js';
import nodeEntryUtils, { setWritingKey, NODE_ENTRY_SIZE } from './utils/nodeEntry.js';
import nodeRoleUtils from './utils/roles.js';
import lengthEntryUtils from './utils/lengthEntry.js';
import transactionUtils from './utils/transaction.js';
import {
    BALANCE_FEE,
    toBalance,
    PERCENT_75,
    PERCENT_50,
    PERCENT_25,
    BALANCE_TO_STAKE,
    BALANCE_ZERO,
    toTerm,
} from './utils/balance.js';
import { safeWriteUInt32BE } from '../../utils/buffer.js';
import deploymentEntryUtils from './utils/deploymentEntry.js';
import { deepCopyBuffer } from '../../utils/buffer.js';
import { Status } from './utils/transaction.js';
import Corestore from 'corestore';

const OVERSIZED_BATCH_PENALTY_MULTIPLIER = BATCH_SIZE;

// TODO: #addWriter, #removeWriter, #transfer, #transferFeeTxOperation need to be refactored to get in arguments actor's nodeEntries in buffer format.

class State extends ReadyResource {
    #base;
    #bee;
    #store;
    #wallet;
    #writingKey;
    #config

    /**
     * @param {Corestore} store
     * @param {PeerWallet} wallet
     * @param {object} config
     **/
    constructor(store, wallet, config) {
        super();

        this.#config = config

        this.#store = store;
        this.#wallet = wallet;

        this.check = new Check(config);
        this.#base = new Autobase(this.#store, this.#config.bootstrap, {
            ackInterval: ACK_INTERVAL,
            valueEncoding: AUTOBASE_VALUE_ENCODING,
            bigBatches: false,
            optimistic: false,
            open: this.#setupHyperbee.bind(this),
            apply: this.applyHandler,
        })
    }

    get base() {
        return this.#base;
    }

    get writingKey() {
        return this.#writingKey;
    }

    get applyHandler() {
        return this.#apply.bind(this);
    }

    async _open() {
        console.log("State initialization...")
        await this.#base.ready();
        this.#writingKey = this.#base.local.key;
    }

    async _close() {
        console.log("State: closing gracefully...");
        if (this.#bee !== null) {
            await this.#bee.close();
        }
        await sleep(100);

        if (this.#base !== null) {
            await this.#base.close();
        }
        await sleep(100);

    }

    isWritable() {
        return this.#base.writable;
    }

    isIndexer() {
        return this.#base.isIndexer;
    }

    getUnsignedLength() {
        return this.#base.view.core.length;
    }

    getSignedLength() {
        return this.#base.view.core.signedLength;
    }

    getFee() {
        return transactionUtils.FEE;
    }

    async get(key) {
        const result = await this.#base.view.get(key);
        if (result === null) return null;
        return result.value;
    }

    async getSigned(key) {
        const view_session = this.#base.view.checkout(this.#base.view.core.signedLength);
        try {
            const result = await view_session.get(key);
            return result ? result.value : null;
        } finally {
            await view_session.close();
        }
    }

    async getAdminEntry() {
        const adminEntry = await this.getSigned(EntryType.ADMIN);
        return adminEntry ? adminEntryUtils.decode(adminEntry, this.#config.addressPrefix) : null;
    }

    async getNodeEntry(address) {
        const nodeEntry = await this.getSigned(address);
        return nodeEntry ? nodeEntryUtils.decode(nodeEntry) : null;
    }

    async getNodeEntryUnsigned(address) {
        const nodeEntry = await this.get(address);
        return nodeEntry ? nodeEntryUtils.decode(nodeEntry) : null;
    }

    async allowedToValidate(address) {
        const localWritable = this.isWritable(); // signed
        const localIndexer = this.isIndexer(); // signed

        const unsignedNodeEntry = await this.getNodeEntryUnsigned(address);
        if (!unsignedNodeEntry) return false;

        const unsignedIsWriter = unsignedNodeEntry.isWriter;
        const unsignedIsIndexer = unsignedNodeEntry.isIndexer;

        return !!(localWritable && !localIndexer) && (unsignedIsWriter && !unsignedIsIndexer)
    }

    async isAdminAllowedToValidate() {
        const isAdmin = this.writingKey.toString('hex') === this.#config.bootstrap.toString('hex');
        const isIndexer = this.isIndexer();
        const lengthCondition = await this.getWriterLength() <= MAX_WRITERS_FOR_ADMIN_INDEXER_CONNECTION;
        return !!(isAdmin && isIndexer && lengthCondition);
    }

    async isAddressWhitelisted(address) {
        const nodeEntry = await this.getNodeEntry(address);
        if (nodeEntry === null) return false;
        return !!nodeEntry.isWhitelisted;
    }

    async isAddressWriter(address) {
        const nodeEntry = await this.getNodeEntry(address);
        if (nodeEntry === null) return false;
        return !!nodeEntry.isWriter;
    }

    async isAddressIndexer(address) {
        const nodeEntry = await this.getNodeEntry(address);
        if (nodeEntry === null) return false;
        return !!nodeEntry.isIndexer;
    }

    async getIndexersEntry() {
        return Object.values(this.#base.system.indexers);
    }

    async isWkInIndexersEntry(wk) {
        if (wk === null) return false;
        const indexerListHasWk = Object.values(this.#base.system.indexers)
            .some(entry => b4a.equals(entry.key, wk));
        return indexerListHasWk;
    }

    async getWriterLength() {
        const writersLength = await this.getSigned(EntryType.WRITERS_LENGTH);
        return writersLength ? lengthEntryUtils.decodeBE(writersLength) : null;
    }

    // Not using it, but figured it might spark an idea for a cli command for licenses count or some util.
    async getLicenseCount() {
        const licenseLength = await this.getSigned(EntryType.LICENSE_COUNT);
        return licenseLength ? lengthEntryUtils.decodeBE(licenseLength) : null;
    }

    async getAddressByLicenseId(licenseId) {
        const address = await this.getSigned(EntryType.LICENSE_INDEX + licenseId);
        return address ? addressUtils.bufferToAddress(address, this.#config.addressPrefix) : null;
    }

    async getWriterIndex(index) {
        if (index < 0 || index > Number.MAX_SAFE_INTEGER) return null;
        const writerPublicKey = await this.getSigned(EntryType.WRITERS_INDEX + index);
        return writerPublicKey ? writerPublicKey : null;
    }

    async getRegisteredBootstrapEntry(bootstrap) {
        if (!bootstrap || !isHexString(bootstrap) || bootstrap.length !== 64) return null;
        return await this.getSigned(EntryType.DEPLOYMENT + bootstrap);
    }

    async getRegisteredBootstrapEntryUnsigned(bootstrap) {
        if (!bootstrap || !isHexString(bootstrap) || bootstrap.length !== 64) return null;
        return await this.get(EntryType.DEPLOYMENT + bootstrap);
    }

    async append(payload) {
        await this.#base.append(payload);
    }

    async getIndexerSequenceState() {
        const buf = []
        for (const indexer of Object.values(this.#base.system.indexers)) {
            buf.push(indexer.key);
        }
        return await PeerWallet.blake3(b4a.concat(buf));
    }

    async isInitalizationDisabled() {
        // Retrieve the flag to verify if initialization is allowed
        let initialization = await this.getSigned(EntryType.INITIALIZATION);

        if (initialization === null) {
            return false
        } else {
            return b4a.equals(initialization, safeWriteUInt32BE(0, 0))
        }
    }
    async getTransactionConfirmedLength(hash) {
        if (!isHexString(hash) || hash.length !== 64) {
            throw new Error("Invalid hash format");
        }

        const confirmedLength = this.getSignedLength();
        const historyStream = this.#base.view.createHistoryStream({
            gte: 0,
            lte: confirmedLength
        });

        for await (const entry of historyStream) {
            if (isTransactionRecordPut(entry) && entry.key === hash) {
                return entry.seq;
            }
        }
        return null;
    }

    async confirmedTransactionsBetween(startSignedLength, endSignedLength) {
        if (!Number.isInteger(startSignedLength) || !Number.isInteger(endSignedLength)) {
            throw new Error("Params must be integer");
        }

        if (startSignedLength < 0 || endSignedLength < 0) {
            throw new Error("Params must be non-negative");
        }

        if (startSignedLength > endSignedLength) {
            throw new Error("endSignedLength must be greater than or equal to startSignedLength");
        }

        if (startSignedLength === endSignedLength) return [];

        const currentSignedLength = this.getSignedLength();
        const signedLength2End = Math.min(currentSignedLength, endSignedLength);

        const startSeq = startSignedLength;
        const endSeq = signedLength2End - 1;

        if (startSeq > endSeq) {
            throw new Error("Invalid range");
        }

        const historyStream = this.#base.view.createHistoryStream({
            gte: startSeq,
            lte: endSeq
        });

        const filters = (entry) => {
            const isPut = entry.type === "put";
            const isHex = isHexString(entry.key);
            const is64 = entry.key.length === 64;
            return isPut && isHex && is64;
        }

        let hashes = [];
        for await (const entry of historyStream) {
            if (filters(entry)) {
                hashes.push({ hash: entry.key, confirmed_length: entry.seq });
            }
        }

        return hashes;
    }

    async getRegisteredWriterKey(writingKey) {
        const entry = await this.get(EntryType.WRITER_ADDRESS + writingKey);
        return entry ? addressUtils.addressToBuffer(entry, this.#config.addressPrefix) : null;
    }

    #setupHyperbee(store) {
        this.#bee = new Hyperbee(store.get(TRAC_NAMESPACE), {
            extension: false,
            keyEncoding: HYPERBEE_KEY_ENCODING,
            valueEncoding: HYPERBEE_VALUE_ENCODING,
        })
        return this.#bee;
    }

    // ATTENTION: DO NOT USE METHODS ABOVE IN APPLY PART!
    ///////////////////////////////APPLY////////////////////////////////////

    async #apply(nodes, view, base) {
        const batch = view.batch();
        const batchInvoker = nodes[0].from.key;


        if (nodes.length > BATCH_SIZE) {
            await this.#validatorPenaltyApply(batchInvoker, batch, base, OVERSIZED_BATCH_PENALTY_MULTIPLIER);
            await batch.flush();
            await batch.close();
            return;
        }

        let invalidOperations = 0;

        for (const node of nodes) {

            if (b4a.byteLength(node.value) > transactionUtils.MAXIMUM_OPERATION_PAYLOAD_SIZE) {
                this.#safeLogApply("Node payload exceeds the maximum operation payload size.", node.from.key)
                invalidOperations++;
                continue;
            };

            const op = safeDecodeApplyOperation(node.value);

            if (!op) {
                this.#safeLogApply("Failed to decode operation.", node.from.key)
                invalidOperations++;
                continue;
            }

            const handler = this.#getApplyOperationHandler(op.type);

            if (handler) {
                const result = await handler(op, view, base, node, batch);
                if (result === Status.FAILURE) {
                    invalidOperations++;
                } else if (result === Status.IGNORE) {
                    continue;
                } else if (result !== Status.SUCCESS) {
                    this.#safeLogApply(`Unknown operation status: ${result}`, node.from.key);
                    invalidOperations++;
                }
            } else {
                this.#safeLogApply(`Unknown operation type: ${op.type}`, node.from.key)
                invalidOperations++;
            }
        }
        if (invalidOperations > 0) {
            await this.#validatorPenaltyApply(batchInvoker, batch, base, invalidOperations);
            this.#safeLogApply(`Applied with ${invalidOperations} invalid operations.`)
        }

        await batch.flush();
        await batch.close();
    }

    #getApplyOperationHandler(type) {
        const handlers = {
            [OperationType.BALANCE_INITIALIZATION]: this.#handleApplyInitializeBalanceOperation.bind(this),
            [OperationType.DISABLE_INITIALIZATION]: this.#handleApplyDisableBalanceInitializationOperation.bind(this),
            [OperationType.ADD_ADMIN]: this.#handleApplyAddAdminOperation.bind(this),
            [OperationType.APPEND_WHITELIST]: this.#handleApplyAppendWhitelistOperation.bind(this),
            [OperationType.ADD_WRITER]: this.#handleApplyAddWriterOperation.bind(this),
            [OperationType.REMOVE_WRITER]: this.#handleApplyRemoveWriterOperation.bind(this),
            [OperationType.ADMIN_RECOVERY]: this.#handleApplyAdminRecoveryOperation.bind(this),
            [OperationType.ADD_INDEXER]: this.#handleApplyAddIndexerOperation.bind(this),
            [OperationType.REMOVE_INDEXER]: this.#handleApplyRemoveIndexerOperation.bind(this),
            [OperationType.BAN_VALIDATOR]: this.#handleApplyBanValidatorOperation.bind(this),
            [OperationType.BOOTSTRAP_DEPLOYMENT]: this.#handleApplyBootstrapDeploymentOperation.bind(this),
            [OperationType.TX]: this.#handleApplyTxOperation.bind(this),
            [OperationType.TRANSFER]: this.#handleApplyTransferOperation.bind(this),
        };
        return handlers[type] || null;
    }

    async #handleApplyInitializeBalanceOperation(op, view, base, node, batch) {
        if (!this.check.validateBalanceInitialization(op)) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Contract schema validation failed.", node.from.key)
            return Status.FAILURE;
        };

        // Extract and validate the requester network address
        const adminAddressBuffer = op.address;
        const adminAddressString = addressUtils.bufferToAddress(adminAddressBuffer, this.#config.addressPrefix);
        if (adminAddressString === null) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Requester address is invalid.", node.from.key)
            return Status.FAILURE;
        }

        // Verify requester admin public key
        const requesterAdminPublicKey = PeerWallet.decodeBech32mSafe(adminAddressString);
        if (requesterAdminPublicKey === null) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Error while decoding requester public key.", node.from.key)
            return Status.FAILURE;
        };

        // Validate recipient address
        const recipientAddress = op.bio.ia;
        const recipientAddressString = addressUtils.bufferToAddress(recipientAddress, this.#config.addressPrefix);
        if (recipientAddressString === null) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Recipient address is invalid.", node.from.key)
            return Status.FAILURE;
        };

        // Validate recipient public key
        const recipientPublicKey = PeerWallet.decodeBech32mSafe(recipientAddressString);
        if (recipientPublicKey === null) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Failed to decode recipient public key.", node.from.key)
            return Status.FAILURE;
        };

        // Verify that the amount is not zero
        const amount = toBalance(op.bio.am);
        if (amount === null) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Invalid balance.", node.from.key)
            return Status.FAILURE;
        };

        // Entry has been disabled so there is nothing to do
        if (await this.#isApplyInitalizationDisabled(batch)) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Balance initialization is disabled.", node.from.key)
            return Status.FAILURE;
        };

        // Ensure that an admin invoked this operation
        const adminEntry = await this.#getEntryApply(EntryType.ADMIN, batch);
        const decodedAdminEntry = adminEntryUtils.decode(adminEntry, this.#config.addressPrefix);

        if (decodedAdminEntry === null) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Failed to decode admin entry.", node.from.key)
            return Status.FAILURE;
        }

        if (!this.#isAdminApply(decodedAdminEntry, node)) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Node is not allowed to perform this operation. (ADMIN ONLY)", node.from.key)
            return Status.FAILURE;
        };

        const adminPublicKey = PeerWallet.decodeBech32mSafe(decodedAdminEntry.address);
        if (adminPublicKey === null) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Failed to decode admin public key.", node.from.key)
            return Status.FAILURE;
        };

        // Admin consistency check
        if (!b4a.equals(adminPublicKey, requesterAdminPublicKey)) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "System admin and node public keys do not match.", node.from.key)
            return Status.FAILURE;
        }

        // Recreate requester message
        const message = createMessage(
            this.#config.networkId,
            op.bio.txv,
            op.bio.ia,
            amount.value,
            op.bio.in,
            OperationType.BALANCE_INITIALIZATION
        );
        if (message.length === 0) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Invalid requester message.", node.from.key)
            return Status.FAILURE;
        };

        const hash = await PeerWallet.blake3Safe(message);
        const txHashHexString = op.bio.tx.toString('hex');
        if (!b4a.equals(hash, op.bio.tx)) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Message hash does not match the tx_hash.", node.from.key)
            return Status.FAILURE;
        };

        // Verify signature
        const isMessageVerifed = this.#wallet.verify(op.bio.is, hash, adminPublicKey);
        if (!isMessageVerifed) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        };

        // Verify tx validity - prevent deferred execution attack
        const indexersSequenceState = await this.#getIndexerSequenceStateApply(base);
        if (indexersSequenceState === null) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Indexer sequence state is invalid.", node.from.key)
            return Status.FAILURE;
        };

        if (!b4a.equals(op.bio.txv, indexersSequenceState)) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Transaction was not executed.", node.from.key)
            return Status.FAILURE;
        };

        // Check if the operation has already been applied
        const opEntry = await this.#getEntryApply(txHashHexString, batch);
        if (opEntry !== null) {
            this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Operation has already been applied.", node.from.key)
            return Status.FAILURE;
        };

        let nodeEntry = null;
        const incomingAddressNodeEntryBuffer = await this.#getEntryApply(recipientAddressString, batch);

        if (incomingAddressNodeEntryBuffer === null) {
            nodeEntry = nodeEntryUtils.init(ZERO_WK, nodeRoleUtils.NodeRole.READER, amount.value)
            if (nodeEntry.length === 0) {
                this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Failed to initialize node entry.", node.from.key)
                return Status.FAILURE;
            }

        } else {
            nodeEntry = amount.update(incomingAddressNodeEntryBuffer)
            if (nodeEntry === null) {
                this.#safeLogApply(OperationType.BALANCE_INITIALIZATION, "Failed to set node entry balance.", node.from.key)
                return Status.FAILURE;
            }
        };

        await batch.put(recipientAddressString, nodeEntry);
        await batch.put(txHashHexString, node.value);
        return Status.SUCCESS;
    }

    async #handleApplyDisableBalanceInitializationOperation(op, view, base, node, batch) {
        if (!this.check.validateCoreAdminOperation(op)) {
            this.#safeLogApply(OperationType.DISABLE_INITIALIZATION, "Schema validation failed.", node.from.key)
            return Status.FAILURE;
        };

        // Entry has been disabled so there is nothing to do
        if (await this.#isApplyInitalizationDisabled(batch)) {
            this.#safeLogApply(OperationType.DISABLE_INITIALIZATION, "Balance initialization already disabled.", node.from.key)
            return Status.FAILURE;
        };

        // Extract and validate the network address
        const adminAddressBuffer = op.address;
        const adminAddressString = addressUtils.bufferToAddress(adminAddressBuffer, this.#config.addressPrefix);
        if (adminAddressString === null) {
            this.#safeLogApply(OperationType.DISABLE_INITIALIZATION, "Failed to validate requester address.", node.from.key)
            return Status.FAILURE;
        };

        // Validate requester admin public key
        const requesterAdminPublicKey = PeerWallet.decodeBech32mSafe(adminAddressString);
        if (requesterAdminPublicKey === null) {
            this.#safeLogApply(OperationType.DISABLE_INITIALIZATION, "Failed to decode requester public key.", node.from.key)
            return Status.FAILURE;
        };

        // Ensure that an admin invoked this operation
        const adminEntry = await this.#getEntryApply(EntryType.ADMIN, batch);
        const decodedAdminEntry = adminEntryUtils.decode(adminEntry, this.#config.addressPrefix);

        if (decodedAdminEntry === null) {
            this.#safeLogApply(OperationType.DISABLE_INITIALIZATION, "Failed to decode admin entry.", node.from.key)
            return Status.FAILURE;
        }

        if (!this.#isAdminApply(decodedAdminEntry, node)) {
            this.#safeLogApply(OperationType.DISABLE_INITIALIZATION, "Node is not allowed to perform this operation. (ADMIN ONLY)", node.from.key)
            return Status.FAILURE;
        };

        const adminPublicKey = PeerWallet.decodeBech32mSafe(decodedAdminEntry.address);
        if (adminPublicKey === null) {
            this.#safeLogApply(OperationType.DISABLE_INITIALIZATION, "Failed to decode admin public key.", node.from.key)
            return Status.FAILURE;
        };

        // Admin consistency check
        if (!b4a.equals(adminPublicKey, requesterAdminPublicKey)) {
            this.#safeLogApply(OperationType.DISABLE_INITIALIZATION, "System admin and node public keys do not match.", node.from.key)
            return Status.FAILURE;
        };

        // Recreate requester message
        const message = createMessage(
            this.#config.networkId,
            op.cao.txv,
            op.cao.iw,
            op.cao.in,
            OperationType.DISABLE_INITIALIZATION
        );
        if (message.length === 0) {
            this.#safeLogApply(OperationType.DISABLE_INITIALIZATION, "Invalid requester message.", node.from.key)
            return Status.FAILURE;
        };

        const hash = await PeerWallet.blake3Safe(message);
        const txHashHexString = op.cao.tx.toString('hex');
        if (!b4a.equals(hash, op.cao.tx)) {
            this.#safeLogApply(OperationType.DISABLE_INITIALIZATION, "Message hash does not match the tx_hash.", node.from.key)
            return Status.FAILURE;
        };

        // Verify signature
        const isMessageVerifed = this.#wallet.verify(op.cao.is, hash, adminPublicKey);
        if (!isMessageVerifed) {
            this.#safeLogApply(OperationType.DISABLE_INITIALIZATION, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        };

        // Verify tx validity - prevent deferred execution attack
        const indexersSequenceState = await this.#getIndexerSequenceStateApply(base);
        if (indexersSequenceState === null) {
            this.#safeLogApply(OperationType.DISABLE_INITIALIZATION, "Indexer sequence state is invalid.", node.from.key)
            return Status.FAILURE;
        };

        if (!b4a.equals(op.cao.txv, indexersSequenceState)) {
            this.#safeLogApply(OperationType.DISABLE_INITIALIZATION, "Transaction was not executed.", node.from.key)
            return Status.FAILURE;
        };

        // Check if the operation has already been applied
        const opEntry = await this.#getEntryApply(txHashHexString, batch);
        if (opEntry !== null) {
            this.#safeLogApply(OperationType.DISABLE_INITIALIZATION, "Operation has already been applied.", node.from.key)
            return Status.FAILURE;
        };

        await batch.put(EntryType.INITIALIZATION, safeWriteUInt32BE(0, 0));
        await batch.put(txHashHexString, node.value);

        return Status.SUCCESS;
    }

    async #handleApplyAddAdminOperation(op, view, base, node, batch) {
        /*
            ADD ADMIN OPERATION INITIALIZES THE NETWORK. THIS OPERATION CAN BE PERFORMED ONLY ONCE, AND THE NETWORK CREATOR
            DOES NOT HAVE TO PAY A FEE IN THIS CASE. ATTENTION: IF ANY VALIDATOR ATTEMPTS THIS OPERATION AFTER THE NETWORK
            INITIALIZATION, THEIR STAKED BALANCE WILL BE REDUCED (PUNISHMENT).
        */

        if (!this.check.validateCoreAdminOperation(op)) {
            this.#safeLogApply(OperationType.ADD_ADMIN, "Contract schema validation failed.", node.from.key)
            return Status.FAILURE;
        };

        // Extract and validate the requester address (admin)
        const adminAddressBuffer = op.address;
        const adminAddressString = addressUtils.bufferToAddress(adminAddressBuffer, this.#config.addressPrefix);
        if (adminAddressString === null) {
            this.#safeLogApply(OperationType.ADD_ADMIN, "Requester address is invalid.", node.from.key)
            return Status.FAILURE;
        };

        // Validate requester admin public key (admin)
        const adminPublicKey = PeerWallet.decodeBech32mSafe(adminAddressString);
        if (adminPublicKey === null) {
            this.#safeLogApply(OperationType.ADD_ADMIN, "Error while decoding requester public key.", node.from.key)
            return Status.FAILURE;
        };

        // Check if the operation is being performed by the bootstrap node - the original deployer of the Trac Network
        if (!b4a.equals(node.from.key, this.#config.bootstrap) || !b4a.equals(op.cao.iw, this.#config.bootstrap)) {
            this.#safeLogApply(OperationType.ADD_ADMIN, "Node is not a bootstrap node.", node.from.key)
            return Status.FAILURE;
        };

        // recreate requester message
        const requesterMessage = createMessage(
            this.#config.networkId,
            op.cao.txv,
            op.cao.iw,
            op.cao.in,
            OperationType.ADD_ADMIN
        );

        if (requesterMessage.length === 0) {
            this.#safeLogApply(OperationType.ADD_ADMIN, "Invalid requester message.", node.from.key)
            return Status.FAILURE;
        };

        const hash = await PeerWallet.blake3Safe(requesterMessage);
        if (!b4a.equals(hash, op.cao.tx)) {
            this.#safeLogApply(OperationType.ADD_ADMIN, "Message hash does not match the tx_hash.", node.from.key)
            return Status.FAILURE;
        };

        // verify signature
        const isMessageVerifed = this.#wallet.verify(op.cao.is, op.cao.tx, adminPublicKey)
        const txHashHexString = op.cao.tx.toString('hex');
        if (!isMessageVerifed) {
            this.#safeLogApply(OperationType.ADD_ADMIN, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        };

        // verify tx validity - prevent deferred execution attack
        const indexersSequenceState = await this.#getIndexerSequenceStateApply(base);
        if (indexersSequenceState === null) {
            this.#safeLogApply(OperationType.ADD_ADMIN, "Indexer sequence state is invalid.", node.from.key)
            return Status.FAILURE;
        };

        if (!b4a.equals(op.cao.txv, indexersSequenceState)) {
            this.#safeLogApply(OperationType.ADD_ADMIN, "Transaction was not executed.", node.from.key)
            return Status.FAILURE;
        };

        // Operation will be performed only once, for consistency check verify that the writer key does not exist
        // writer key should NOT exists for a brand new admin
        const writerKeyHasBeenRegistered = await this.#getRegisteredWriterKeyApply(batch, op.cao.iw.toString('hex'))
        if (writerKeyHasBeenRegistered !== null) {
            this.#safeLogApply(OperationType.ADD_ADMIN, "Writer key already exists.", node.from.key)
            return Status.FAILURE;
        };

        const adminEntryExists = await this.#getEntryApply(EntryType.ADMIN, batch);
        // if admin entry already exists, cannot perform this operation
        if (adminEntryExists !== null) {
            this.#safeLogApply(OperationType.ADD_ADMIN, "Admin entry already exists.", node.from.key)
            return Status.FAILURE;
        };

        // Check if the operation has already been applied
        const opEntry = await this.#getEntryApply(txHashHexString, batch);
        if (opEntry !== null) {
            this.#safeLogApply(OperationType.ADD_ADMIN, "Operation has already been applied.", node.from.key)
            return Status.FAILURE;
        };

        const { newLicenseLength, decodedNewLicenseLength } = await this.#applyAssignNewLicense(batch);
        if (newLicenseLength !== null && decodedNewLicenseLength) {
            await batch.put(EntryType.LICENSE_COUNT, newLicenseLength)
            await batch.put(EntryType.LICENSE_INDEX + decodedNewLicenseLength, adminAddressBuffer)
        } else {
            // This log should (if this error ever happend) ALWAYS log.
            this.#safeLogApply("SYSTEM ERROR", "Something went wrong while updating license index.", node.from.key)
        }

        const initializedNodeEntry = nodeEntryUtils.init(op.cao.iw, nodeRoleUtils.NodeRole.INDEXER, ADMIN_INITIAL_BALANCE, newLicenseLength, ADMIN_INITIAL_STAKED_BALANCE);
        if (initializedNodeEntry.length === 0) {
            this.#safeLogApply(OperationType.ADD_ADMIN, "Failed to initialize node entry.", node.from.key)
            return Status.FAILURE;
        }

        // Create a new admin entry
        const newAdminEntry = adminEntryUtils.encode(adminAddressBuffer, op.cao.iw, this.#config.addressPrefix);
        if (newAdminEntry.length === 0) {
            this.#safeLogApply(OperationType.ADD_ADMIN, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        };

        await batch.put(adminAddressString, initializedNodeEntry);
        await batch.put(EntryType.WRITER_ADDRESS + op.cao.iw.toString('hex'), op.address);

        const { length, incrementedLength } = await this.#updateWritersIndex(batch);

        if (length !== null && incrementedLength !== null) {
            // Update the writers index and length entries  
            await batch.put(EntryType.WRITERS_INDEX + length, adminAddressBuffer);
            await batch.put(EntryType.WRITERS_LENGTH, incrementedLength);
        } else {
            // This log should (if this error ever happend) ALWAYS log.
            this.#safeLogApply("SYSTEM ERROR", "Something went wrong while updating writers index.", node.from.key)
        }

        // initialize admin entry and initialization flag
        await batch.put(EntryType.ADMIN, newAdminEntry);
        await batch.put(EntryType.INITIALIZATION, safeWriteUInt32BE(1, 0));
        await batch.put(txHashHexString, node.value);

        if (this.#config.enableTxApplyLogs) {
            console.info(`Admin added addr:wk:tx - ${adminAddressString}:${op.cao.iw.toString('hex')}:${txHashHexString}`);
        }

        return Status.SUCCESS;
    }

    async #handleApplyAdminRecoveryOperation(op, view, base, node, batch) {
        if (!this.check.validateRoleAccessOperation(op)) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Contract schema validation failed.", node.from.key)
            return Status.FAILURE;
        };

        // if transaction is not complete, do not process it.
        if (!Object.hasOwn(op.rao, "vs") || !Object.hasOwn(op.rao, "va") || !Object.hasOwn(op.rao, "vn")) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Operation is not complete.", node.from.key)
            return Status.FAILURE;
        };

        // for additional security, nonces should be different.
        if (b4a.equals(op.rao.in, op.rao.vn)) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Nonces should not be the same.", node.from.key)
            return Status.FAILURE;
        };

        // addresses should be different.
        if (b4a.equals(op.address, op.rao.va)) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Addresses should be different.", node.from.key)
            return Status.FAILURE;
        };

        // signatures should be different.
        if (b4a.equals(op.rao.is, op.rao.vs)) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Signatures should be different.", node.from.key)
            return Status.FAILURE;
        };

        // Extract and validate the requester address and pubkey
        const requesterAdminAddressBuffer = op.address;
        const requesterAdminAddressString = addressUtils.bufferToAddress(requesterAdminAddressBuffer, this.#config.addressPrefix);
        if (requesterAdminAddressString === null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Requester address is invalid.", node.from.key)
            return Status.FAILURE;
        };

        const requesterAdminPublicKey = PeerWallet.decodeBech32mSafe(requesterAdminAddressString);
        if (requesterAdminPublicKey === null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Error while decoding requester public key.", node.from.key)
            return Status.FAILURE;
        };

        // recreate requester message
        const requesterMessage = createMessage(
            this.#config.networkId,
            op.rao.txv,
            op.rao.iw,
            op.rao.in,
            OperationType.ADMIN_RECOVERY
        );

        if (requesterMessage.length === 0) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Invalid requester message.", node.from.key)
            return Status.FAILURE;
        };

        const hash = await PeerWallet.blake3Safe(requesterMessage);
        if (!b4a.equals(hash, op.rao.tx)) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Message hash does not match the tx_hash.", node.from.key)
            return Status.FAILURE;
        };

        // verify requester signature
        const isRequesterMessageVerifed = this.#wallet.verify(op.rao.is, op.rao.tx, requesterAdminPublicKey);
        const txHashHexString = op.rao.tx.toString('hex');
        if (!isRequesterMessageVerifed) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Failed to verify requester message signature.", node.from.key)
            return Status.FAILURE;
        };

        // Extract and validate the validator address and pubkey
        const validatorAddress = op.rao.va;
        const validatorAddressString = addressUtils.bufferToAddress(validatorAddress, this.#config.addressPrefix);
        if (validatorAddressString === null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Failed to validate validator address.", node.from.key)
            return Status.FAILURE;
        };

        const validatorPublicKey = PeerWallet.decodeBech32mSafe(validatorAddressString);
        if (validatorPublicKey === null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Failed to decode validator public key.", node.from.key)
            return Status.FAILURE;
        };

        // recreate validator message
        const validatorMessage = createMessage(
            this.#config.networkId,
            op.rao.tx,
            op.rao.vn,
            OperationType.ADMIN_RECOVERY
        );

        if (validatorMessage.length === 0) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Failed to verify validator message signature.", node.from.key)
            return Status.FAILURE;
        };

        // verify validator signature
        const validatorHash = await PeerWallet.blake3Safe(validatorMessage);
        const isValidatorMessageVerifed = this.#wallet.verify(op.rao.vs, validatorHash, validatorPublicKey);
        if (!isValidatorMessageVerifed) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        };

        // The writer key must NOT be linked to any address since this is an ADMIN recovery.
        // Until the next release with indexer rotation, we simply enforce the new writer key.
        const writerKeyHasBeenRegistered = await this.#getRegisteredWriterKeyApply(batch, op.rao.iw.toString('hex'))
        if (writerKeyHasBeenRegistered !== null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Writer key already exists.", node.from.key)
            return Status.FAILURE;
        };

        // verify tx validity - prevent deferred execution attack
        const indexersSequenceState = await this.#getIndexerSequenceStateApply(base);
        if (indexersSequenceState === null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Indexer sequence state is invalid.", node.from.key)
            return Status.FAILURE;
        };
        if (!b4a.equals(op.rao.txv, indexersSequenceState)) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Transaction was not executed.", node.from.key)
            return Status.FAILURE;
        };

        const validatorEntryBuffer = await this.#getEntryApply(validatorAddressString, batch);

        // Validator consistency checks
        const isValidatorValid = await this.#isValidatorValidApply(validatorEntryBuffer, node, op);
        if (!isValidatorValid) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Validator consistency check failed.", node.from.key)
            return Status.FAILURE;
        }

        const adminEntry = await this.#getEntryApply(EntryType.ADMIN, batch);
        const decodedAdminEntry = adminEntryUtils.decode(adminEntry, this.#config.addressPrefix);

        if (decodedAdminEntry === null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Failed to decode admin entry.", node.from.key)
            return Status.FAILURE;
        };

        const publicKeyAdminEntry = PeerWallet.decodeBech32mSafe(decodedAdminEntry.address);
        if (!b4a.equals(requesterAdminPublicKey, publicKeyAdminEntry)) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Admin public key does not match the node public key.", node.from.key)
            return Status.FAILURE;
        };

        // anti-replay attack
        // NOTE: We would honestly keep this failure because in theory this should never happen.
        const opEntry = await this.#getEntryApply(txHashHexString, batch);
        if (opEntry !== null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Operation has already been applied.", node.from.key)
            return Status.FAILURE;
        };

        const isOldWkInIndexerList = await this.#isWriterKeyInIndexerListApply(decodedAdminEntry.wk, base);
        if (!isOldWkInIndexerList) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Old writer key is not in indexer list.", node.from.key)
            return Status.FAILURE;
        }; // Old admin wk is not in indexers entry

        // Update admin entry with new writing key
        const newAdminEntry = adminEntryUtils.encode(requesterAdminAddressBuffer, op.rao.iw, this.#config.addressPrefix);
        if (newAdminEntry.length === 0) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Invalid admin entry.", node.from.key)
            return Status.FAILURE;
        };

        // Update node entry of the admin with new writing key
        const adminNodeEntry = await this.#getEntryApply(requesterAdminAddressString, batch);
        const newAdminNodeEntry = setWritingKey(adminNodeEntry, op.rao.iw)

        const isNewWkInIndexerList = await this.#isWriterKeyInIndexerListApply(op.rao.iw, base);
        if (isNewWkInIndexerList) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "New writer key is already in indexer list.", node.from.key)
            return Status.FAILURE;
        }; // New admin wk is already in indexers entry

        // charging fee from the requester (admin)
        const decodedAdminNodeEntry = nodeEntryUtils.decode(newAdminNodeEntry, this.#config.addressPrefix)
        if (decodedAdminNodeEntry === null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Failed to decode node entry.", node.from.key)
            return Status.FAILURE;
        }

        const adminBalance = toBalance(decodedAdminNodeEntry.balance)
        if (adminBalance === null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Invalid admin balance.", node.from.key)
            return Status.FAILURE;
        }

        if (!adminBalance.greaterThanOrEquals(BALANCE_FEE)) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Insufficient admin balance.", node.from.key)
            return Status.IGNORE;
        };
        const updatedFee = adminBalance.sub(BALANCE_FEE)

        if (updatedFee === null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Failed to apply fee.", node.from.key)
            return Status.FAILURE;
        }
        const chargedAdminEntry = updatedFee.update(newAdminNodeEntry)

        // Reward logic
        const validatorNodeEntry = nodeEntryUtils.decode(validatorEntryBuffer, this.#config.addressPrefix);
        if (validatorNodeEntry === null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Invalid validator node entry.", node.from.key)
            return Status.FAILURE;
        };

        const validatorBalance = toBalance(validatorNodeEntry.balance);
        if (validatorBalance === null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Invalid validator balance.", node.from.key)
            return Status.FAILURE;
        };

        const newValidatorBalance = validatorBalance.add(BALANCE_FEE.percentage(PERCENT_75));
        if (newValidatorBalance === null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Failed to transfer fee to validator.", node.from.key)
            return Status.FAILURE;
        };

        const updatedValidatorNodeEntry = newValidatorBalance.update(validatorEntryBuffer)
        if (updatedValidatorNodeEntry === null) {
            this.#safeLogApply(OperationType.ADMIN_RECOVERY, "Failed to update validator balance.", node.from.key)
            return Status.FAILURE;
        };

        // Revoke old wk and add new one as an indexer
        await base.removeWriter(decodedAdminEntry.wk);
        await base.addWriter(op.rao.iw, { indexer: true });
        await batch.put(EntryType.WRITER_ADDRESS + op.rao.iw.toString('hex'), op.address);

        // Remove the old admin entry and add the new one
        await batch.put(EntryType.ADMIN, newAdminEntry);
        // This updates the admin node entry with the new writer key and deducted fee.
        await batch.put(requesterAdminAddressString, chargedAdminEntry);

        // Actually pay the fee
        await batch.put(validatorAddressString, updatedValidatorNodeEntry);
        await batch.put(txHashHexString, node.value);

        if (this.#config.enableTxApplyLogs) {
            console.info(`Admin has been recovered addr:wk:tx - ${requesterAdminAddressString}:${op.rao.iw.toString('hex')}:${txHashHexString}`);
        }

        return Status.SUCCESS;
    }

    async #handleApplyAppendWhitelistOperation(op, view, base, node, batch) {
        if (!this.check.validateAdminControlOperation(op)) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Contract schema validation failed.", node.from.key)
            return Status.FAILURE;
        };

        // Validate the recipient address
        const adminAddressBuffer = op.address;
        const adminAddressString = addressUtils.bufferToAddress(adminAddressBuffer, this.#config.addressPrefix);
        if (adminAddressString === null) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Recipient address is invalid.", node.from.key)
            return Status.FAILURE;
        };
        // Validate recipient public key
        const requesterAdminPublicKey = PeerWallet.decodeBech32mSafe(adminAddressString);
        if (requesterAdminPublicKey === null) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Failed to decode recipient public key.", node.from.key)
            return Status.FAILURE;
        };

        // Retrieve and decode the admin entry to verify the operation is initiated by an admin
        const adminEntry = await this.#getEntryApply(EntryType.ADMIN, batch);
        if (adminEntry === null) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Failed to verify admin entry.", node.from.key)
            return Status.FAILURE;
        };

        const decodedAdminEntry = adminEntryUtils.decode(adminEntry, this.#config.addressPrefix);
        if (decodedAdminEntry === null) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Failed to decode admin entry.", node.from.key)
            return Status.FAILURE;
        }

        if (!this.#isAdminApply(decodedAdminEntry, node)) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Node is not allowed to perform this operation. (ADMIN ONLY)", node.from.key)
            return Status.FAILURE;
        };

        // Extract admin entry
        const adminAddress = decodedAdminEntry.address;
        const adminPublicKey = PeerWallet.decodeBech32mSafe(adminAddress);
        if (adminPublicKey === null) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Failed to decode admin public key.", node.from.key)
            return Status.FAILURE;
        };

        //admin consistency check
        if (!b4a.equals(adminPublicKey, requesterAdminPublicKey)) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "System admin and node public keys do not match.", node.from.key)
            return Status.FAILURE;
        };

        // Extract and validate the network prefix from the node's address
        const nodeAddressBuffer = op.aco.ia;

        const nodeAddressString = addressUtils.bufferToAddress(nodeAddressBuffer, this.#config.addressPrefix);
        if (nodeAddressString === null) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Failed to verify node address.", node.from.key)
            return Status.FAILURE;
        };
        const nodePublicKey = PeerWallet.decodeBech32mSafe(nodeAddressString);
        if (nodePublicKey === null) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Failed to decode node public key.", node.from.key)
            return Status.FAILURE;
        };

        // verify signature
        const message = createMessage(
            this.#config.networkId,
            op.aco.txv,
            op.aco.ia,
            op.aco.in,
            OperationType.APPEND_WHITELIST
        );
        if (message.length === 0) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Invalid requester message.", node.from.key)
            return Status.FAILURE;
        };

        // verify signature
        const hash = await PeerWallet.blake3Safe(message);
        if (!b4a.equals(hash, op.aco.tx)) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Message hash does not match the tx_hash.", node.from.key)
            return Status.FAILURE;
        };

        const isMessageVerified = this.#wallet.verify(op.aco.is, op.aco.tx, adminPublicKey);
        if (!isMessageVerified) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        };

        const hashHexString = op.aco.tx.toString('hex');

        // verify tx validity - prevent deferred execution attack
        const indexersSequenceState = await this.#getIndexerSequenceStateApply(base);
        if (indexersSequenceState === null) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Indexer sequence state is invalid.", node.from.key)
            return Status.FAILURE;
        };

        if (!b4a.equals(op.aco.txv, indexersSequenceState)) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Transaction was not executed.", node.from.key)
            return Status.FAILURE;
        };

        // Check if the operation has already been applied
        const opEntry = await this.#getEntryApply(hashHexString, batch);
        if (opEntry !== null) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Operation has already been applied.", node.from.key)
            return Status.FAILURE;
        };

        // Retrieve the node entry to check its current role
        const nodeEntry = await this.#getEntryApply(nodeAddressString, batch);
        if (nodeEntryUtils.isWhitelisted(nodeEntry)) {
            this.#safeLogApply(OperationType.APPEND_WHITELIST, "Node already whitelisted.", node.from.key)
            return Status.FAILURE;
        }; // Node is already whitelisted

        if (await this.#isApplyInitalizationDisabled(batch)) {
            // Fee
            const adminNodeEntry = await this.#getEntryApply(adminAddressString, batch);
            if (adminNodeEntry === null) {
                this.#safeLogApply(OperationType.APPEND_WHITELIST, "Failed to validate admin entry.", node.from.key)
                return Status.FAILURE;
            };

            const decodedNodeEntry = nodeEntryUtils.decode(adminNodeEntry, this.#config.addressPrefix)
            if (decodedNodeEntry === null) {
                this.#safeLogApply(OperationType.APPEND_WHITELIST, "Failed to decode admin entry.", node.from.key)
                return Status.FAILURE;
            };

            const adminBalance = toBalance(decodedNodeEntry.balance)
            if (adminBalance === null) {
                this.#safeLogApply(OperationType.APPEND_WHITELIST, "Invalid admin balance.", node.from.key)
                return Status.FAILURE;
            };

            if (!adminBalance.greaterThanOrEquals(BALANCE_FEE)) {
                this.#safeLogApply(OperationType.APPEND_WHITELIST, "Insufficient admin balance.", node.from.key)
                return Status.FAILURE;
            };
            const newAdminBalance = adminBalance.sub(BALANCE_FEE)

            if (newAdminBalance === null) {
                this.#safeLogApply(OperationType.APPEND_WHITELIST, "Failed to apply fee to admin balance.", node.from.key)
                return Status.FAILURE;
            };
            const updatedAdminEntry = newAdminBalance.update(adminNodeEntry)

            if (updatedAdminEntry === null) {
                this.#safeLogApply(OperationType.APPEND_WHITELIST, "Failed to update admin entry.", node.from.key)
                return Status.FAILURE;
            };

            await batch.put(adminAddressString, updatedAdminEntry);
        }

        if (!nodeEntry) {
            // If the node entry does not exist, create a new whitelisted node entry
            /*
                Dear reader,
                wk = 00000000000000000000000000000000 on ed25519 is point P.
                P = (19681161376707505956807079304988542015446066515923890162744021073123829784752,0).
                This point belongs to the curve but is not a valid point.
                Point P belongs to the torsion subgroup E(Fp)_TOR of the curve.

                Yes, you could theoretically (easily) forge a signature on this point.
                No, you don’t need to worry about it.

                Why? Because `wk` is only used as an identifier in our network:
                1. Trac pair of keys is higher in hierarchy.
                2. Our network leverages Libsodium, a robust cryptographic library that enforces stringent checks:
                    - Anyone attempting to create a node with such a key won't be able to participate in our network.
                    - If an attacker tries to use a small order key, signature
                    verification fails due to checks that reject such keys;
                    - The cofactor is always cleared when generating keys,
                    thanks to a process called clamping, which forces private keys
                    to lie in the prime-order subgroup by fixing certain bits.
                    This protects against attacks involving small-order points;
                3. Even if you are assigned this specific wk (the all-zero identifier), you can rest assured
                that you won't be able to perform any network actions with it. You can only directly participate
                in the network if you possess a valid wk. As an indirect user, this characteristic doesn't affect you.

            */
            // If node does not exist, then create a new licence. 
            const { newLicenseLength, decodedNewLicenseLength } = await this.#applyAssignNewLicense(batch);
            if (newLicenseLength !== null && decodedNewLicenseLength) {
                await batch.put(EntryType.LICENSE_COUNT, newLicenseLength)
                await batch.put(EntryType.LICENSE_INDEX + decodedNewLicenseLength, nodeAddressBuffer)
            } else {
                // This log should (if this error ever happend) ALWAYS log.
                this.#safeLogApply("SYSTEM ERROR", "Something went wrong while updating license index.", node.from.key)
            }

            const initializedNodeEntry = nodeEntryUtils.init(ZERO_WK, nodeRoleUtils.NodeRole.WHITELISTED, nodeRoleUtils.ZERO_BALANCE, newLicenseLength);
            if (initializedNodeEntry.length === 0) {
                this.#safeLogApply(OperationType.APPEND_WHITELIST, "Failed to initialize node entry.", node.from.key)
                return Status.FAILURE;
            }

            await batch.put(nodeAddressString, initializedNodeEntry);
            await batch.put(hashHexString, node.value);
        } else {
            // If the node entry exists, update its role to WHITELISTED. Case if account will buy license from market but it existed before - for example it had balance.
            // I assume since we dont have a marketplace now, that we by default assign a new license to any whitelisted node.

            const decodedNodeEntry = nodeEntryUtils.decode(nodeEntry);
            if (decodedNodeEntry === null) {
                this.#safeLogApply(OperationType.APPEND_WHITELIST, "Failed to decode node entry.", node.from.key)
                return Status.FAILURE;
            };
            const editedNodeEntry = nodeEntryUtils.setRole(nodeEntry, nodeRoleUtils.NodeRole.WHITELISTED);

            if (editedNodeEntry === null) {
                this.#safeLogApply(OperationType.APPEND_WHITELIST, "Failed to edit node entry.", node.from.key)
                return Status.FAILURE;
            }

            // Edge case: if the user license is not ZERO_LICENSE, then we do not assign a new license. 
            // This means the admin has decided to unban the node. 
            // This is important because if the admin mistakenly whitelists a node that already has a license, 
            // the previous license could be overwritten and lost permanently. 
            // Therefore, in this case we do not overwrite the license — we only change the role.
            if (!b4a.equals(decodedNodeEntry.license, nodeEntryUtils.ZERO_LICENSE)) {
                await batch.put(nodeAddressString, editedNodeEntry);

            } else {
                const { newLicenseLength, decodedNewLicenseLength } = await this.#applyAssignNewLicense(batch);
                if (newLicenseLength !== null && decodedNewLicenseLength) {
                    await batch.put(EntryType.LICENSE_COUNT, newLicenseLength)
                    await batch.put(EntryType.LICENSE_INDEX + decodedNewLicenseLength, nodeAddressBuffer)
                } else {
                    // This log should (if this error ever happend) ALWAYS log.
                    this.#safeLogApply("SYSTEM ERROR", "Something went wrong while updating license index.", node.from.key)
                }

                const nodeEntryWithNewLicense = nodeEntryUtils.setLicense(editedNodeEntry, newLicenseLength)
                await batch.put(nodeAddressString, nodeEntryWithNewLicense);
            }

            await batch.put(hashHexString, node.value);
        }
        // Only whitelisted node will be able to become a writer/indexer.
        return Status.SUCCESS;
    }

    async #handleApplyAddWriterOperation(op, view, base, node, batch) {
        if (!this.check.validateRoleAccessOperation(op)) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Contract schema validation failed.", node.from.key)
            return Status.FAILURE;
        };

        // if transaction is not complete, do not process it.
        if (!Object.hasOwn(op.rao, "vs") || !Object.hasOwn(op.rao, "va") || !Object.hasOwn(op.rao, "vn")) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Operation is not complete.", node.from.key)
            return Status.FAILURE;
        };

        // for additional security, nonces should be different.
        if (b4a.equals(op.rao.in, op.rao.vn)) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Nonces should not be the same.", node.from.key)
            return Status.FAILURE;
        };

        // addresses should be different.
        if (b4a.equals(op.address, op.rao.va)) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Addresses should be different.", node.from.key)
            return Status.FAILURE;
        };

        // signatures should be different.
        if (b4a.equals(op.rao.is, op.rao.vs)) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Signatures should be different.", node.from.key)
            return Status.FAILURE;
        };

        // Extract and validate the requester address
        const requesterAddressBuffer = op.address;
        const requesterAddressString = addressUtils.bufferToAddress(requesterAddressBuffer, this.#config.addressPrefix);
        if (requesterAddressString === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Requester address is invalid.", node.from.key)
            return Status.FAILURE;
        };

        const requesterPublicKey = PeerWallet.decodeBech32mSafe(requesterAddressString);
        if (requesterPublicKey === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Error while decoding requester public key.", node.from.key)
            return Status.FAILURE;
        };

        // if node want to register ZERO_WK, then this is NOT ALLOWED
        if (b4a.equals(op.rao.iw, ZERO_WK)) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Writer cannot initialize with zero-writer-key.", node.from.key)
            return Status.FAILURE;
        };

        // verify requester signature
        const requesterMessage = createMessage(
            this.#config.networkId,
            op.rao.txv,
            op.rao.iw,
            op.rao.in,
            OperationType.ADD_WRITER
        );

        if (requesterMessage.length === 0) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Invalid requester message.", node.from.key)
            return Status.FAILURE;
        };

        const hash = await PeerWallet.blake3Safe(requesterMessage);
        if (!b4a.equals(hash, op.rao.tx)) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Message hash does not match the tx_hash.", node.from.key)
            return Status.FAILURE;
        };

        const isRequesterMessageVerifed = this.#wallet.verify(op.rao.is, op.rao.tx, requesterPublicKey);
        const txHashHexString = op.rao.tx.toString('hex');
        if (!isRequesterMessageVerifed) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        };

        // verify validator signature
        const validatorAddress = op.rao.va;
        const validatorAddressString = addressUtils.bufferToAddress(validatorAddress, this.#config.addressPrefix);
        if (validatorAddressString === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Failed to validate validator address.", node.from.key)
            return Status.FAILURE;
        };

        // validate validator public key
        const validatorPublicKey = PeerWallet.decodeBech32mSafe(validatorAddressString);
        if (validatorPublicKey === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Failed to decode validator public key.", node.from.key)
            return Status.FAILURE;
        };

        // recreate validator message
        const validatorMessage = createMessage(
            this.#config.networkId,
            op.rao.tx,
            op.rao.vn,
            OperationType.ADD_WRITER
        );

        if (validatorMessage.length === 0) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Invalid validator message.", node.from.key)
            return Status.FAILURE;
        };

        const validatorHash = await PeerWallet.blake3Safe(validatorMessage);
        const isValidatorMessageVerifed = this.#wallet.verify(op.rao.vs, validatorHash, validatorPublicKey);
        if (!isValidatorMessageVerifed) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Failed to verify validator message signature.", node.from.key)
            return Status.FAILURE;
        };

        // verify tx validity - prevent deferred execution attack
        const indexersSequenceState = await this.#getIndexerSequenceStateApply(base);
        if (indexersSequenceState === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Indexer sequence state is invalid.", node.from.key)
            return Status.FAILURE;
        };

        if (!b4a.equals(op.rao.txv, indexersSequenceState)) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Transaction was not executed.", node.from.key)
            return Status.FAILURE;
        };

        const validatorEntryBuffer = await this.#getEntryApply(validatorAddressString, batch);

        // Validator consistency checks
        const isValidatorValid = await this.#isValidatorValidApply(validatorEntryBuffer, node, op);
        if (!isValidatorValid) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Validator consistency check failed.", node.from.key)
            return Status.FAILURE;
        }

        // anti-replay attack
        const opEntry = await this.#getEntryApply(txHashHexString, batch);
        if (opEntry !== null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Operation has already been applied.", node.from.key)
            return Status.IGNORE;
        };

        const addWriterResult = await this.#addWriter(op, base, node, batch, txHashHexString, requesterAddressString, requesterAddressBuffer, validatorAddressString, validatorEntryBuffer);
        if (addWriterResult === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Failed to add writer.", node.from.key)
            return Status.FAILURE;
        }

        if (addWriterResult === Status.IGNORE) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Add writer operation ignored.", node.from.key)
            return Status.IGNORE;
        }
        return Status.SUCCESS;
    }

    async #addWriter(op, base, node, batch, txHashHexString, requesterAddressString, requesterAddressBuffer, validatorAddressString, validatorEntryBuffer) {
        // Retrieve the node entry for the given address, if null then do not process...
        const requesterNodeEntry = await this.#getEntryApply(requesterAddressString, batch);
        if (requesterNodeEntry === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Failed to verify requester node address.", node.from.key)
            return null;
        };

        const decodedRequesterNodeEntry = nodeEntryUtils.decode(requesterNodeEntry, this.#config.addressPrefix)
        if (decodedRequesterNodeEntry === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Failed to decode node entry.", node.from.key)
            return null;
        };

        /*
            Writer Key Validation Cases:
          
            Case 1: New Writing Key (writerKeyHasBeenRegistered === null)
            - If the key has never been registered before
            - System will register this new key and link it to the requester's address
            - Always allowed as long as other conditions are met (whitelisting, balance, etc.)
          
            Case 2: Previously Used Key (writerKeyHasBeenRegistered !== null)
            Two conditions must be met:
            a) Key Match (isCurrentWk):
                - The key must be the same as currently assigned in node's entry
                - Prevents using different keys than what's assigned
            
            b) Ownership (isOwner):
                - The requester must be the original owner of this key
                - Enables re-staking after being downgraded to reader
                - Prevents key usage by non-owners
          
            This validation ensures:
            1. Only legitimate new keys are registered
            2. Downgraded nodes can re-stake using their original keys
            3. Keys cannot be reused by different addresses
         */

        const writerKeyHasBeenRegistered = await this.#getRegisteredWriterKeyApply(batch, op.rao.iw.toString('hex'))
        if (writerKeyHasBeenRegistered !== null) {
            const isCurrentWk = b4a.equals(decodedRequesterNodeEntry.wk, op.rao.iw);
            const isOwner = b4a.equals(writerKeyHasBeenRegistered, requesterAddressBuffer);

            if (!isCurrentWk || !isOwner) {
                this.#safeLogApply(OperationType.ADD_WRITER, "Invalid writer key: either not owned by requester or different from assigned key.", node.from.key)
                return null;
            }
        }

        const isWhitelisted = decodedRequesterNodeEntry.isWhitelisted
        const isWriter = decodedRequesterNodeEntry.isWriter;
        const isIndexer = decodedRequesterNodeEntry.isIndexer;

        // To become a writer the node must be whitelisted and not already a writer or indexer
        if (isIndexer || isWriter || !isWhitelisted) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Node must be whitelisted, and cannot be a writer or an indexer.", node.from.key)
            return null;
        };

        // Charging fee from the requester
        const requesterBalance = toBalance(decodedRequesterNodeEntry.balance)
        if (requesterBalance === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Invalid requester balance.", node.from.key)
            return null;
        };

        if (!requesterBalance.greaterThanOrEquals(BALANCE_FEE)) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Insufficient requester balance.", node.from.key)
            return Status.IGNORE;
        };

        const updatedBalance = requesterBalance.sub(BALANCE_FEE) // Remove the fee
        if (updatedBalance === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Failed to apply fee to requester balance.", node.from.key)
            return null;
        };

        // Update the node entry to assign the writer role and deduct the fee from the requester's balance
        const updatedRoleRequesterNodeEntry = nodeEntryUtils.setRoleAndWriterKey(requesterNodeEntry, nodeRoleUtils.NodeRole.WRITER, op.rao.iw);
        if (updatedRoleRequesterNodeEntry === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Failed to update node entry with a writer role.", node.from.key)
            return null;
        };

        const chargedFeeRequesterNodeEntry = updatedBalance.update(updatedRoleRequesterNodeEntry)
        if (chargedFeeRequesterNodeEntry === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Failed to update node balance.", node.from.key)
            return null;
        };

        // reward the validator

        const decodedValidatorEntry = nodeEntryUtils.decode(validatorEntryBuffer, this.#config.addressPrefix)
        if (decodedValidatorEntry === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Failed to decode validator entry.", node.from.key)
            return null;
        };

        const validatorBalance = toBalance(decodedValidatorEntry.balance)
        if (validatorBalance === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Invalid validator balance.", node.from.key)
            return null;
        };

        const updatedValidatorBalance = validatorBalance.add(BALANCE_FEE.percentage(PERCENT_75))
        if (updatedValidatorBalance === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Failed to transfer fee to validator.", node.from.key)
            return null;
        };

        const updatedValidatorEntry = updatedValidatorBalance.update(validatorEntryBuffer)
        if (updatedValidatorEntry === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Failed to update validator entry.", node.from.key)
            return null;
        };

        const finalRequesterNodeEntry = this.#stakeBalanceApply(chargedFeeRequesterNodeEntry, node);
        if (finalRequesterNodeEntry === null) {
            this.#safeLogApply(OperationType.ADD_WRITER, "Failed to stake balance for writer.", node.from.key)
            return null;
        };

        // Add the writer role to the base and update the batch
        await base.addWriter(op.rao.iw, { isIndexer: false });
        await batch.put(requesterAddressString, finalRequesterNodeEntry);

        if (writerKeyHasBeenRegistered === null) {
            await batch.put(EntryType.WRITER_ADDRESS + op.rao.iw.toString('hex'), op.address);
        }

        const { length, incrementedLength } = await this.#updateWritersIndex(batch);

        if (length !== null && incrementedLength !== null) {
            // Update the writers index and length entries
            await batch.put(EntryType.WRITERS_INDEX + length, requesterAddressBuffer);
            await batch.put(EntryType.WRITERS_LENGTH, incrementedLength);
        } else {
            // This log should (if this error ever happend) ALWAYS log.
            this.#safeLogApply("SYSTEM ERROR", "Something went wrong while updating writers index.", node.from.key)
        }

        // Pay the fee to the validator
        await batch.put(validatorAddressString, updatedValidatorEntry);
        await batch.put(txHashHexString, node.value);

        if (this.#config.enableTxApplyLogs) {
            console.info(`Writer has been added addr:wk:tx - ${requesterAddressString}:${op.rao.iw.toString('hex')}:${txHashHexString}`);
        }
    }

    async #handleApplyRemoveWriterOperation(op, view, base, node, batch) {
        if (!this.check.validateRoleAccessOperation(op)) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Contract schema validation failed.", node.from.key)
            return Status.FAILURE;
        };

        // if transaction is not complete, do not process it.
        if (!Object.hasOwn(op.rao, "vs") || !Object.hasOwn(op.rao, "va") || !Object.hasOwn(op.rao, "vn")) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Operation is not complete.", node.from.key)
            return Status.FAILURE;
        };

        // for additional security, nonces should be different.
        if (b4a.equals(op.rao.in, op.rao.vn)) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Nonces should not be the same.", node.from.key)
            return Status.FAILURE;
        };

        // addresses should be different.
        if (b4a.equals(op.address, op.rao.va)) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Addresses should be different.", node.from.key)
            return Status.FAILURE;
        };

        // signatures should be different.
        if (b4a.equals(op.rao.is, op.rao.vs)) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Signatures should be different.", node.from.key)
            return Status.FAILURE;
        };

        // Extract and validate the network address
        const requesterAddress = op.address;
        const requesterAddressString = addressUtils.bufferToAddress(requesterAddress, this.#config.addressPrefix);
        if (requesterAddressString === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Requester address is invalid.", node.from.key)
            return Status.FAILURE;
        };

        // Validate requester public key
        const requesterPublicKey = PeerWallet.decodeBech32mSafe(requesterAddressString);
        if (requesterPublicKey === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Error while decoding requester public key.", node.from.key)
            return Status.FAILURE;
        };

        // verify requester signature
        const requesterMessage = createMessage(
            this.#config.networkId,
            op.rao.txv,
            op.rao.iw,
            op.rao.in,
            OperationType.REMOVE_WRITER
        );
        if (requesterMessage.length === 0) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Invalid requester message.", node.from.key)
            return Status.FAILURE;
        };

        // compare hashes
        const hash = await PeerWallet.blake3Safe(requesterMessage);
        if (!b4a.equals(hash, op.rao.tx)) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Message hash does not match the tx_hash.", node.from.key)
            return Status.FAILURE;
        };

        const isRequesterMessageVerifed = this.#wallet.verify(op.rao.is, op.rao.tx, requesterPublicKey);
        const txHashHexString = op.rao.tx.toString('hex');
        if (!isRequesterMessageVerifed) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        };

        // verify validator signature
        const validatorAddress = op.rao.va;
        const validatorAddressString = addressUtils.bufferToAddress(validatorAddress, this.#config.addressPrefix);
        if (validatorAddressString === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Failed to verify validator address.", node.from.key)
            return Status.FAILURE;
        };

        // validate validator public key
        const validatorPublicKey = PeerWallet.decodeBech32mSafe(validatorAddressString);
        if (validatorPublicKey === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Failed to decode validator public key.", node.from.key)
            return Status.FAILURE;
        };

        // recreate validator message
        const validatorMessage = createMessage(
            this.#config.networkId,
            op.rao.tx,
            op.rao.vn,
            OperationType.REMOVE_WRITER
        );
        if (validatorMessage.length === 0) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Invalid validator message.", node.from.key)
            return Status.FAILURE;
        };

        const validatorHash = await PeerWallet.blake3Safe(validatorMessage);
        const isValidatorMessageVerifed = this.#wallet.verify(op.rao.vs, validatorHash, validatorPublicKey);
        if (!isValidatorMessageVerifed) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Failed to verify validator message signature.", node.from.key)
            return Status.FAILURE;
        };

        // verify tx validity - prevent deferred execution attack
        const indexersSequenceState = await this.#getIndexerSequenceStateApply(base);
        if (indexersSequenceState === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Indexer sequence state is invalid.", node.from.key)
            return Status.FAILURE;
        };

        if (!b4a.equals(op.rao.txv, indexersSequenceState)) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Transaction was not executed.", node.from.key)
            return Status.FAILURE;
        };

        const validatorEntryBuffer = await this.#getEntryApply(validatorAddressString, batch);

        // Validator consistency checks
        const isValidatorValid = await this.#isValidatorValidApply(validatorEntryBuffer, node, op);
        if (!isValidatorValid) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Validator consistency check failed.", node.from.key)
            return Status.FAILURE;
        };

        // anti-replay attack
        const opEntry = await this.#getEntryApply(txHashHexString, batch);
        if (opEntry !== null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Operation has already been applied.", node.from.key)
            return Status.IGNORE;
        };

        // Proceed to remove the writer role from the node
        const removeWriterResult = await this.#removeWriter(op, base, node, batch, txHashHexString, requesterAddressString, requesterAddress, validatorAddressString, validatorEntryBuffer);
        if (removeWriterResult === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Failed to remove writer.", node.from.key)
            return Status.FAILURE;
        }

        if (removeWriterResult === Status.IGNORE) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Remove writer operation ignored.", node.from.key)
            return Status.IGNORE;
        }

        return Status.SUCCESS;
    }

    async #removeWriter(op, base, node, batch, txHashHexString, requesterAddressString, requesterAddress, validatorAddressString, validatorEntryBuffer) {

        // Fetch the node entry for the given address
        const requesterNodeEntry = await this.#getEntryApply(requesterAddressString, batch);
        if (requesterNodeEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Failed to verify requester node entry.", node.from.key)
            return null;
        };

        const decodedNodeEntry = nodeEntryUtils.decode(requesterNodeEntry, this.#config.addressPrefix);
        if (decodedNodeEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Failed to decode requester node entry.", node.from.key)
            return null;
        };

        // Check if the node is a writer or an indexer
        const isNodeWriter = decodedNodeEntry.isWriter;
        const isNodeIndexer = decodedNodeEntry.isIndexer;

        if (isNodeIndexer || !isNodeWriter) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Node has to be a writer, and cannot be an indexer.", node.from.key)
            return null;
        };

        /**
         * Ensure that: 
         * 1) writer key exists in registry (we can not unregister something that was not registered), 
         * 2) matches the one in node entry , 
         * 3) belongs to the requester - this prevents unauthorized key removal
         */
        const writerKeyHasBeenRegistered = await this.#getRegisteredWriterKeyApply(batch, op.rao.iw.toString('hex'))
        if (writerKeyHasBeenRegistered === null ||
            !b4a.equals(op.rao.iw, decodedNodeEntry.wk) ||
            !b4a.equals(writerKeyHasBeenRegistered, requesterAddress)
        ) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Writer key must be registered, match node's current key, and belong to the requester.", node.from.key)
            return null;
        }

        // Charging fee from the requester
        const requesterBalance = toBalance(decodedNodeEntry.balance);
        if (requesterBalance === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Invalid requester balance.", node.from.key)
            return null;
        };

        if (!requesterBalance.greaterThanOrEquals(BALANCE_FEE)) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Insufficient requester balance.", node.from.key)
            return Status.IGNORE;
        };

        const updatedBalance = requesterBalance.sub(BALANCE_FEE);
        if (updatedBalance === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Failed to apply fee to requester balance.", node.from.key)
            return null;
        };

        // Downgrade role from WRITER to WHITELISTED and deduct the fee from the requester's balance
        const updatedNodeEntry = nodeEntryUtils.setRole(requesterNodeEntry, nodeRoleUtils.NodeRole.WHITELISTED);
        if (updatedNodeEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Failed to update node entry role.", node.from.key)
            return null;
        };
        const chargedNodeEntry = updatedBalance.update(updatedNodeEntry);
        if (chargedNodeEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Failed to update node balance.", node.from.key)
            return null;
        };

        // Validator reward logic 
        const decodedValidatorEntry = nodeEntryUtils.decode(validatorEntryBuffer, this.#config.addressPrefix);
        if (decodedValidatorEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Failed to decode validator node entry.", node.from.key)
            return null;
        };

        const validatorBalance = toBalance(decodedValidatorEntry.balance)
        if (validatorBalance === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Invalid validator balance.", node.from.key)
            return null;
        };

        const validatorNewBalance = validatorBalance.add(BALANCE_FEE.percentage(PERCENT_75))
        if (validatorNewBalance === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Failed to transfer fee to validator balance.", node.from.key)
            return null;
        };

        const updateValidatorEntry = validatorNewBalance.update(validatorEntryBuffer)
        if (updateValidatorEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Failed to update validator balance.", node.from.key)
            return null;
        };

        const finalRequesterNodeEntry = this.#withdrawStakedBalanceApply(chargedNodeEntry, node);
        if (finalRequesterNodeEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_WRITER, "Failed to unstake balance for writer.", node.from.key)
            return null;
        };

        // Remove the writer role and update the state
        await base.removeWriter(decodedNodeEntry.wk);
        await batch.put(requesterAddressString, finalRequesterNodeEntry);

        // Reward the validator
        await batch.put(validatorAddressString, updateValidatorEntry);
        await batch.put(txHashHexString, node.value);

        if (this.#config.enableTxApplyLogs) {
            console.info(`Writer removed: addr:wk:tx - ${requesterAddressString}:${op.rao.iw.toString('hex')}:${txHashHexString}`);
        }

        this.#emitEvent(CustomEventType.UNWRITABLE, PeerWallet.decodeBech32mSafe(requesterAddressString))
    }

    async #handleApplyAddIndexerOperation(op, view, base, node, batch) {
        if (!this.check.validateAdminControlOperation(op)) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Contract schema validation failed.", node.from.key)
            return Status.FAILURE;
        };

        // Extract and validate the requester address (admin)
        const requesterAddressBuffer = op.address;
        const requesterAddressString = addressUtils.bufferToAddress(requesterAddressBuffer, this.#config.addressPrefix);
        if (requesterAddressString === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Requester address is invalid.", node.from.key)
            return Status.FAILURE;
        };

        // Validate requester public key
        const requesterPublicKey = PeerWallet.decodeBech32mSafe(requesterAddressString);
        if (requesterPublicKey === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Error while decoding requester public key.", node.from.key)
            return Status.FAILURE;
        };

        // Extract and validate pretending indexer address
        const pretendingAddressBuffer = op.aco.ia;
        const pretendingAddressString = addressUtils.bufferToAddress(pretendingAddressBuffer, this.#config.addressPrefix);
        if (pretendingAddressString === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Pretending indexer address is invalid.", node.from.key)
            return Status.FAILURE;
        };

        // Validate pretending indexer public key
        const pretentingPublicKey = PeerWallet.decodeBech32mSafe(pretendingAddressString);
        if (pretentingPublicKey === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Failed to decode pretending indexer public key.", node.from.key)
            return Status.FAILURE;
        };

        // ensure that an admin invoked this operation
        const adminEntry = await this.#getEntryApply(EntryType.ADMIN, batch);
        if (adminEntry === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Invalid admin entry.", node.from.key)
            return Status.FAILURE;
        };

        const decodedAdminEntry = adminEntryUtils.decode(adminEntry, this.#config.addressPrefix);
        if (decodedAdminEntry === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Failed to decode admin entry.", node.from.key)
            return Status.FAILURE;
        };

        if (!this.#isAdminApply(decodedAdminEntry, node)) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Node is not allowed to perform this operation. (ADMIN ONLY)", node.from.key)
            return Status.FAILURE;
        };

        // Extract admin public key 
        const adminPublicKey = PeerWallet.decodeBech32mSafe(decodedAdminEntry.address);
        if (adminPublicKey === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Failed to decode admin public key.", node.from.key)
            return Status.FAILURE;
        };

        // Admin consistency check
        if (!b4a.equals(adminPublicKey, requesterPublicKey)) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "System admin and node public keys do not match.", node.from.key)
            return Status.FAILURE;
        };

        // verify requester signature
        const message = createMessage(
            this.#config.networkId,
            op.aco.txv,
            op.aco.ia,
            op.aco.in,
            OperationType.ADD_INDEXER
        );

        if (message.length === 0) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Invalid requester message.", node.from.key)
            return Status.FAILURE;
        };

        const hash = await PeerWallet.blake3Safe(message);
        if (!b4a.equals(hash, op.aco.tx)) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Message hash does not match the tx_hash.", node.from.key)
            return Status.FAILURE;
        };

        const isMessageVerifed = this.#wallet.verify(op.aco.is, hash, adminPublicKey);
        const txHashHexString = hash.toString('hex');
        if (!isMessageVerifed) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        };

        // verify tx validity - prevent deferred execution attack
        const indexersSequenceState = await this.#getIndexerSequenceStateApply(base);
        if (indexersSequenceState === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Indexer sequence state is invalid.", node.from.key)
            return Status.FAILURE;
        };

        if (!b4a.equals(op.aco.txv, indexersSequenceState)) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Transaction was not executed.", node.from.key)
            return Status.FAILURE;
        };

        // anti-replay attack
        const opEntry = await this.#getEntryApply(txHashHexString, batch);
        if (opEntry !== null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Operation has already been applied.", node.from.key)
            return Status.FAILURE;
        };

        const addIndexerResult = await this.#addIndexer(op, node, batch, base, txHashHexString, pretendingAddressString, requesterAddressString);
        if (addIndexerResult === null) {
            return Status.FAILURE;
        }

        return Status.SUCCESS;
    }

    async #addIndexer(op, node, batch, base, txHashHexString, pretendingAddressString, requesterAddressString) {

        const pretenderNodeEntry = await this.#getEntryApply(pretendingAddressString, batch);
        if (pretenderNodeEntry === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Failed to verify target indexer entry.", node.from.key)
            return null;
        };

        const decodedPretenderNodeEntry = nodeEntryUtils.decode(pretenderNodeEntry, this.#config.addressPrefix);
        if (decodedPretenderNodeEntry === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Failed to decode pretender indexer node entry.", node.from.key)
            return null;
        };

        //check if node is allowed to become an indexer
        const isNodeWriter = nodeEntryUtils.isWriter(pretenderNodeEntry);
        const isNodeIndexer = nodeEntryUtils.isIndexer(pretenderNodeEntry);
        if (!isNodeWriter || isNodeIndexer) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Node must be a writer, and cannot already be an indexer.", node.from.key)
            return null;
        };

        //update node entry to indexer
        const updatedNodeEntry = nodeEntryUtils.setRole(pretenderNodeEntry, nodeRoleUtils.NodeRole.INDEXER)
        if (updatedNodeEntry === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Failed to update node role.", node.from.key)
            return null;
        };

        // ensure that the node wk does not exist in the indexer list
        const indexerListHasWk = await this.#isWriterKeyInIndexerListApply(decodedPretenderNodeEntry.wk, base);
        if (indexerListHasWk) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Writer key already exists in indexer list.", node.from.key)
            return null;
        }; // Wk is already in indexer list (Node already indexer)

        // charge fee from the admin (requester)
        const feeAmount = toBalance(transactionUtils.FEE);
        if (feeAmount === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Invalid fee amount.", node.from.key)
            return null;
        };

        const adminNodeEntryBuffer = await this.#getEntryApply(requesterAddressString, batch);
        if (adminNodeEntryBuffer === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Invalid requester node entry buffer.", node.from.key)
            return null;
        };

        const adminNodeEntry = nodeEntryUtils.decode(adminNodeEntryBuffer, this.#config.addressPrefix);
        if (adminNodeEntry === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Failed to decode requester node entry.", node.from.key)
            return null;
        };

        const adminBalance = toBalance(adminNodeEntry.balance);
        if (adminBalance === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Invalid admin balance.", node.from.key)
            return null;
        };

        if (!adminBalance.greaterThanOrEquals(feeAmount)) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Insufficient requester balance.", node.from.key)
            return null;
        };

        // 100% fee charged from admin will be burned
        const newAdminBalance = adminBalance.sub(feeAmount);
        if (newAdminBalance === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Failed to apply fee to requester balance.", node.from.key)
            return null;
        };

        const updatedAdminNodeEntry = newAdminBalance.update(adminNodeEntryBuffer);
        if (updatedAdminNodeEntry === null) {
            this.#safeLogApply(OperationType.ADD_INDEXER, "Failed to update requester node.", node.from.key)
            return null;
        };

        // set indexer role
        await base.removeWriter(decodedPretenderNodeEntry.wk);
        await base.addWriter(decodedPretenderNodeEntry.wk, { indexer: true })

        // change node entry to indexer and update admin balance after fee deduction
        await batch.put(pretendingAddressString, updatedNodeEntry);
        await batch.put(requesterAddressString, updatedAdminNodeEntry);

        // store operation hash to avoid replay attack.
        await batch.put(txHashHexString, node.value);

        if (this.#config.enableTxApplyLogs) {
            console.info(`Indexer added addr:wk:tx - ${pretendingAddressString}:${decodedPretenderNodeEntry.wk.toString('hex')}:${txHashHexString}`);
        }

        this.#emitEvent(CustomEventType.IS_INDEXER, PeerWallet.decodeBech32mSafe(pretendingAddressString))
    }

    async #handleApplyRemoveIndexerOperation(op, view, base, node, batch) {
        if (!this.check.validateAdminControlOperation(op)) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Contract schema validation failed.", node.from.key)
            return Status.FAILURE;
        };

        // Extract and validate the requester address (admin)
        const requesterAddressBuffer = op.address;
        const requesterAddressString = addressUtils.bufferToAddress(requesterAddressBuffer, this.#config.addressPrefix);
        if (requesterAddressString === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Requester address is invalid.", node.from.key)
            return Status.FAILURE;
        };

        // Validate requester public key (admin)
        const requesterPublicKey = PeerWallet.decodeBech32mSafe(requesterAddressString);
        if (requesterPublicKey === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Error while decoding requester public key.", node.from.key)
            return Status.FAILURE;
        };

        // Extract and validate pretending indexer address
        const toRemoveAddressBuffer = op.aco.ia;
        const toRemoveAddressString = addressUtils.bufferToAddress(toRemoveAddressBuffer, this.#config.addressPrefix);
        if (toRemoveAddressString === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Target indexer address is invalid.", node.from.key)
            return Status.FAILURE;
        };

        const toRemoveAddressPublicKey = PeerWallet.decodeBech32mSafe(toRemoveAddressString);
        if (toRemoveAddressPublicKey === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Failed to decode target indexer public key.", node.from.key)
            return Status.FAILURE;
        };

        // ensure that an admin invoked this operation
        const adminEntry = await this.#getEntryApply(EntryType.ADMIN, batch);
        if (adminEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Invalid admin entry.", node.from.key)
            return Status.FAILURE;
        };

        const decodedAdminEntry = adminEntryUtils.decode(adminEntry, this.#config.addressPrefix);
        if (decodedAdminEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Failed to decode admin entry.", node.from.key)
            return Status.FAILURE;
        };

        if (!this.#isAdminApply(decodedAdminEntry, node)) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Node is not allowed to perform this operation. (ADMIN ONLY)", node.from.key)
            return Status.FAILURE;
        };

        const adminPublicKey = PeerWallet.decodeBech32mSafe(decodedAdminEntry.address);
        if (adminPublicKey === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Failed to decode admin public key.", node.from.key)
            return Status.FAILURE;
        };

        if (!b4a.equals(requesterPublicKey, adminPublicKey)) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "System admin and node public keys do not match.", node.from.key)
            return Status.FAILURE;
        };

        // verify requester signature
        const message = createMessage(
            this.#config.networkId,
            op.aco.txv,
            op.aco.ia,
            op.aco.in,
            OperationType.REMOVE_INDEXER
        );

        if (message.length === 0) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Invalid requester message.", node.from.key)
            return Status.FAILURE;
        };
        // compare hashes
        const hash = await PeerWallet.blake3Safe(message);
        if (!b4a.equals(hash, op.aco.tx)) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Message hash does not match the tx_hash.", node.from.key)
            return Status.FAILURE;
        };

        const isMessageVerifed = this.#wallet.verify(op.aco.is, hash, adminPublicKey);
        const txHashHexString = hash.toString('hex');
        if (!isMessageVerifed) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        };

        // verify tx validity - prevent deferred execution attack
        const indexersSequenceState = await this.#getIndexerSequenceStateApply(base);
        if (indexersSequenceState === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Indexer sequence state is invalid.", node.from.key)
            return Status.FAILURE;
        };

        if (!b4a.equals(op.aco.txv, indexersSequenceState)) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Transaction was not executed.", node.from.key)
            return Status.FAILURE;
        };

        // anti-replay attack
        const opEntry = await this.#getEntryApply(txHashHexString, batch);
        if (opEntry !== null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Operation has already been applied.", node.from.key)
            return Status.FAILURE;
        };

        const removeIndexerResult = await this.#removeIndexer(op, node, batch, base, txHashHexString, toRemoveAddressString, toRemoveAddressBuffer, requesterAddressString);
        if (removeIndexerResult === null) {
            return Status.FAILURE;
        };
        return Status.SUCCESS;
    }

    async #removeIndexer(op, node, batch, base, txHashHexString, toRemoveAddressString, toRemoveAddressBuffer, requesterAddressString) {
        const toRemoveNodeEntry = await this.#getEntryApply(toRemoveAddressString, batch);
        if (toRemoveNodeEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Failed to verify target indexer entry.", node.from.key)
            return null;
        };

        const decodedNodeEntry = nodeEntryUtils.decode(toRemoveNodeEntry, this.#config.addressPrefix);
        if (decodedNodeEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Failed to decode target indexer node entry.", node.from.key)
            return null;
        };

        // Check if the node entry is an indexer
        const isNodeIndexer = nodeEntryUtils.isIndexer(toRemoveNodeEntry);
        if (!isNodeIndexer) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Node must be an indexer.", node.from.key)
            return null;
        };

        //update node entry to writer
        const updatedNodeEntry = nodeEntryUtils.setRoleAndWriterKey(toRemoveNodeEntry, nodeRoleUtils.NodeRole.WRITER, decodedNodeEntry.wk)
        if (updatedNodeEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Failed to update node role.", node.from.key)
            return null;
        };

        // Ensure that the node is an indexer
        const indexerListHasWk = await this.#isWriterKeyInIndexerListApply(decodedNodeEntry.wk, base);
        if (!indexerListHasWk) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Writer key does not exist in indexer list.", node.from.key)
            return null;
        }; // Node is not an indexer.

        // Charging fee from the admin (requester)
        const adminNodeEntry = await this.#getEntryApply(requesterAddressString, batch);
        if (adminNodeEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Invalid requester node entry.", node.from.key)
            return null;
        };

        const decodedAdminNodeEntry = nodeEntryUtils.decode(adminNodeEntry, this.#config.addressPrefix)
        if (decodedAdminNodeEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Failed to decode requester node entry.", node.from.key)
            return null;
        };

        const adminBalance = toBalance(decodedAdminNodeEntry.balance)
        if (adminBalance === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Invalid admin balance.", node.from.key)
            return null;
        };

        if (!adminBalance.greaterThanOrEquals(BALANCE_FEE)) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Insufficient requester balance.", node.from.key)
            return null;
        };

        // 100% fee will be burned
        const newAdminBalance = adminBalance.sub(BALANCE_FEE)
        if (newAdminBalance === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Failed to apply fee to requester balance.", node.from.key)
            return null;
        };

        const updatedAdminNodeEntry = newAdminBalance.update(adminNodeEntry)
        if (updatedAdminNodeEntry === null) {
            this.#safeLogApply(OperationType.REMOVE_INDEXER, "Failed to update requester node.", node.from.key)
            return null;
        };

        // downgrade role to writer
        await base.removeWriter(decodedNodeEntry.wk);
        await base.addWriter(decodedNodeEntry.wk, { isIndexer: false });

        // update writers index and length
        const { length, incrementedLength } = await this.#updateWritersIndex(batch);

        if (length !== null && incrementedLength !== null) {
            // Update the writers index and length entries 
            await batch.put(EntryType.WRITERS_INDEX + length, toRemoveAddressBuffer);
            await batch.put(EntryType.WRITERS_LENGTH, incrementedLength);
        } else {
            // This log should (if this error ever happend) ALWAYS log.
            this.#safeLogApply("SYSTEM ERROR", "Something went wrong while updating writers index.", node.from.key)
        }

        //update node entry and indexers entry
        await batch.put(toRemoveAddressString, updatedNodeEntry);

        // update requester (admin) entry after fee deduction
        await batch.put(requesterAddressString, updatedAdminNodeEntry);

        // store operation hash to avoid replay attack.
        await batch.put(txHashHexString, node.value);
        if (this.#config.enableTxApplyLogs) {
            console.info(`Indexer has been removed addr:wk:tx - ${toRemoveAddressString}:${decodedNodeEntry.wk.toString('hex')}:${txHashHexString}`);
        }
    }

    async #handleApplyBanValidatorOperation(op, view, base, node, batch) {
        if (!this.check.validateAdminControlOperation(op)) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Contract schema validation failed.", node.from.key)
            return Status.FAILURE;
        };
        // Extract and validate the network prefix from the node's address
        const requesterAddressBuffer = op.address;
        const requesterAddressString = addressUtils.bufferToAddress(requesterAddressBuffer, this.#config.addressPrefix);
        if (requesterAddressString === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Requester address is invalid.", node.from.key)
            return Status.FAILURE;
        };

        // Validate requester public key
        const requesterPublicKey = PeerWallet.decodeBech32mSafe(requesterAddressString);
        if (requesterPublicKey === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Error while decoding requester public key.", node.from.key)
            return Status.FAILURE;
        };

        // ensure that an admin invoked this operation
        const adminEntry = await this.#getEntryApply(EntryType.ADMIN, batch);
        if (adminEntry === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Invalid admin entry.", node.from.key)
            return Status.FAILURE;
        };

        const decodedAdminEntry = adminEntryUtils.decode(adminEntry, this.#config.addressPrefix);
        if (decodedAdminEntry === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Failed to decode admin node entry.", node.from.key)
            return Status.FAILURE;
        };

        const adminPublicKey = PeerWallet.decodeBech32mSafe(decodedAdminEntry.address);
        if (adminPublicKey === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Failed to decode admin public key.", node.from.key)
            return Status.FAILURE;
        };

        if (!this.#isAdminApply(decodedAdminEntry, node)) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Node is not allowed to perform this operation. (ADMIN ONLY)", node.from.key)
            return Status.FAILURE;
        };

        // Admin consistency check
        if (!b4a.equals(adminPublicKey, requesterPublicKey)) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "System admin and node public keys do not match.", node.from.key)
            return Status.FAILURE;
        };

        // recreate requester message
        const message = createMessage(
            this.#config.networkId,
            op.aco.txv,
            op.aco.ia,
            op.aco.in,
            OperationType.BAN_VALIDATOR
        );
        if (message.length === 0) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Invalid requester message.", node.from.key)
            return Status.FAILURE;
        };

        // compare hashes
        const regeneratedHash = await PeerWallet.blake3Safe(message);
        if (!b4a.equals(regeneratedHash, op.aco.tx)) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Message hash does not match the tx_hash.", node.from.key)
            return Status.FAILURE;
        };

        const isMessageVerifed = this.#wallet.verify(op.aco.is, regeneratedHash, adminPublicKey);
        const txHashHexString = regeneratedHash.toString('hex');
        if (!isMessageVerifed) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        }

        // verify tx validity - prevent deferred execution attack
        const indexersSequenceState = await this.#getIndexerSequenceStateApply(base);
        if (indexersSequenceState === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Indexer sequence state is invalid.", node.from.key)
            return Status.FAILURE;
        }

        if (!b4a.equals(op.aco.txv, indexersSequenceState)) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Transaction was not executed.", node.from.key)
            return Status.FAILURE;
        };

        // check if the operation has already been applied
        const opEntry = await this.#getEntryApply(txHashHexString, batch);
        if (opEntry !== null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Operation has already been applied.", node.from.key)
            return Status.FAILURE;
        };

        // Extract and validate the node address to be banned
        const nodeToBeBannedAddressBuffer = op.aco.ia;
        const nodeToBeBannedAddressString = addressUtils.bufferToAddress(nodeToBeBannedAddressBuffer, this.#config.addressPrefix);
        if (nodeToBeBannedAddressString === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Failed to verify target node address.", node.from.key)
            return Status.FAILURE;
        };

        const toBanNodeEntry = await this.#getEntryApply(nodeToBeBannedAddressString, batch);
        if (toBanNodeEntry === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Failed to verify target node entry.", node.from.key)
            return Status.FAILURE;
        }; // Node entry must exist to ban it.

        // Atleast writer must be whitelisted to ban it.
        const isWhitelisted = nodeEntryUtils.isWhitelisted(toBanNodeEntry);
        const isWriter = nodeEntryUtils.isWriter(toBanNodeEntry);
        const isIndexer = nodeEntryUtils.isIndexer(toBanNodeEntry);

        // only writer/whitelisted node can be banned.
        if ((!isWhitelisted && !isWriter) || isIndexer) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Only writer/whitelisted node can be banned.", node.from.key)
            return Status.FAILURE;
        };

        const updatedToBanNodeEntry = nodeEntryUtils.setRole(toBanNodeEntry, nodeRoleUtils.NodeRole.READER);
        if (updatedToBanNodeEntry === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Failed to update target node role.", node.from.key)
            return Status.FAILURE;
        };

        const decodedToBanNodeEntry = nodeEntryUtils.decode(updatedToBanNodeEntry, this.#config.addressPrefix);
        if (decodedToBanNodeEntry === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Failed to decode target node entry.", node.from.key)
            return Status.FAILURE;
        };

        // charge fee from the admin
        const feeAmount = toBalance(transactionUtils.FEE);
        if (feeAmount === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Invalid fee amount.", node.from.key)
            return Status.FAILURE;
        };

        const adminNodeEntryBuffer = await this.#getEntryApply(requesterAddressString, batch);
        if (adminNodeEntryBuffer === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Invalid admin node entry buffer.", node.from.key)
            return Status.FAILURE;
        };

        const adminNodeEntry = nodeEntryUtils.decode(adminNodeEntryBuffer, this.#config.addressPrefix);
        if (adminNodeEntry === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Failed to verify admin node entry.", node.from.key)
            return Status.FAILURE;
        };

        const adminBalance = toBalance(adminNodeEntry.balance);
        if (adminBalance === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Invalid admin balance", node.from.key)
            return Status.FAILURE;
        };

        if (!adminBalance.greaterThanOrEquals(feeAmount)) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Insufficient admin balance.", node.from.key)
            return Status.FAILURE;
        };

        // 100% fee charged from admin will be burned
        const newAdminBalance = adminBalance.sub(feeAmount);
        if (newAdminBalance === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Failed to apply fee to admin balance.", node.from.key)
            return Status.FAILURE;
        };

        const updatedAdminNodeEntry = newAdminBalance.update(adminNodeEntryBuffer);
        if (updatedAdminNodeEntry === null) {
            this.#safeLogApply(OperationType.BAN_VALIDATOR, "Failed to update admin node balance.", node.from.key)
            return Status.FAILURE;
        }

        // Remove the writer role and update the state
        if (isWriter) {
            const finalNodeEntry = this.#withdrawStakedBalanceApply(updatedToBanNodeEntry, node);
            if (finalNodeEntry === null) {
                this.#safeLogApply(OperationType.BAN_VALIDATOR, "Failed to withdraw staked balance.", node.from.key)
                return Status.FAILURE;
            }
            await base.removeWriter(decodedToBanNodeEntry.wk);
            await batch.put(nodeToBeBannedAddressString, finalNodeEntry);

        } else {
            await batch.put(nodeToBeBannedAddressString, updatedToBanNodeEntry);
        }

        await batch.put(requesterAddressString, updatedAdminNodeEntry);
        await batch.put(txHashHexString, node.value);
        if (this.#config.enableTxApplyLogs) {
            console.info(`Node has been banned: addr:wk:tx - ${nodeToBeBannedAddressString}:${decodedToBanNodeEntry.wk.toString('hex')}:${txHashHexString}`);
        }

        this.#emitEvent(CustomEventType.UNWRITABLE, PeerWallet.decodeBech32mSafe(nodeToBeBannedAddressString))

        return Status.SUCCESS;
    }

    async #handleApplyBootstrapDeploymentOperation(op, view, base, node, batch) {
        if (!this.check.validateBootstrapDeploymentOperation(op)) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Contract schema validation failed.", node.from.key)
            return Status.FAILURE;
        };
        // if transaction is not complete, do not process it.
        if (!Object.hasOwn(op.bdo, "vs") || !Object.hasOwn(op.bdo, "va") || !Object.hasOwn(op.bdo, "vn")) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Operation is not complete.", node.from.key)
            return Status.FAILURE;
        };
        // do not allow to deploy bootstrap deployment on the same bootstrap.
        if (b4a.equals(op.bdo.bs, this.#config.bootstrap)) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Cannot deploy bootstrap on existing same bootstrap.", node.from.key)
            return Status.FAILURE;
        };
        // for additional security, nonces should be different.
        if (b4a.equals(op.bdo.in, op.bdo.vn)) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Nonces should not be the same.", node.from.key)
            return Status.FAILURE;
        };
        // addresses should be different.
        if (b4a.equals(op.address, op.bdo.va)) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Addresses should be different.", node.from.key)
            return Status.FAILURE;
        };
        // signatures should be different.
        if (b4a.equals(op.bdo.is, op.bdo.vs)) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Signatures should be different.", node.from.key)
            return Status.FAILURE;
        };


        // validate requester signature
        const requesterAddressBuffer = op.address;
        const requesterAddressString = addressUtils.bufferToAddress(requesterAddressBuffer, this.#config.addressPrefix);
        if (requesterAddressString === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Requester address is invalid.", node.from.key)
            return Status.FAILURE;
        };

        // validate requester public key
        const requesterPublicKey = PeerWallet.decodeBech32mSafe(requesterAddressString);
        if (requesterPublicKey === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Failed to decode requester public key.", node.from.key)
            return Status.FAILURE;
        };

        // recreate requester message
        const requesterMessage = createMessage(
            this.#config.networkId,
            op.bdo.txv,
            op.bdo.bs,
            op.bdo.ic,
            op.bdo.in,
            OperationType.BOOTSTRAP_DEPLOYMENT
        );

        if (requesterMessage.length === 0) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Invalid requester message.", node.from.key)
            return Status.FAILURE;
        };

        // ensure that tx is valid
        const regeneratedTxHash = await PeerWallet.blake3Safe(requesterMessage);
        if (!b4a.equals(regeneratedTxHash, op.bdo.tx)) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Message hash does not match the tx_hash.", node.from.key)
            return Status.FAILURE;
        };

        const isRequesterSignatureValid = this.#wallet.verify(op.bdo.is, regeneratedTxHash, requesterPublicKey);
        if (!isRequesterSignatureValid) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        };

        const bootstrapDeploymentHexString = op.bdo.bs.toString('hex');

        //validation of validator signature
        const validatorAddressBuffer = op.bdo.va;
        const validatorAddressString = addressUtils.bufferToAddress(validatorAddressBuffer, this.#config.addressPrefix);
        if (validatorAddressString === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Invalid validator address.", node.from.key)
            return Status.FAILURE;
        };

        // validate validator public key
        const validatorPublicKey = PeerWallet.decodeBech32mSafe(validatorAddressString);
        if (validatorPublicKey === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Failed to decode validator public key.", node.from.key)
            return Status.FAILURE;
        };

        // recreate validator message
        const validatorMessage = createMessage(
            this.#config.networkId,
            op.bdo.tx,
            op.bdo.vn,
            OperationType.BOOTSTRAP_DEPLOYMENT
        );

        if (validatorMessage.length === 0) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Invalid validator message.", node.from.key)
            return Status.FAILURE;
        };

        const validatorMessageHash = await PeerWallet.blake3Safe(validatorMessage);

        const isValidatorSignatureValid = this.#wallet.verify(op.bdo.vs, validatorMessageHash, validatorPublicKey);
        if (!isValidatorSignatureValid) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Failed to verify validator message signature.", node.from.key)
            return Status.FAILURE;
        };

        // verify tx validity - prevent deferred execution attack
        const indexersSequenceState = await this.#getIndexerSequenceStateApply(base);
        if (indexersSequenceState === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Indexer sequence state is invalid.", node.from.key)
            return Status.FAILURE;
        };

        if (!b4a.equals(op.bdo.txv, indexersSequenceState)) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Transaction was not executed.", node.from.key)
            return Status.FAILURE;
        };

        const validatorEntryBuffer = await this.#getEntryApply(validatorAddressString, batch);

        // Validator consistency checks
        const isValidatorValid = await this.#isValidatorValidApply(validatorEntryBuffer, node, op);
        if (!isValidatorValid) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Validator consistency check failed.", node.from.key)
            return Status.FAILURE;
        }

        // anti-replay attack
        const hashHexString = op.bdo.tx.toString('hex');
        const opEntry = await this.#getEntryApply(hashHexString, batch);
        if (opEntry !== null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Operation has already been applied.", node.from.key)
            return Status.IGNORE;
        }; // Operation has already been applied.

        // If deployment already exists, do not process it again.
        const alreadyRegisteredBootstrap = await this.#getDeploymentEntryApply(bootstrapDeploymentHexString, batch);
        if (alreadyRegisteredBootstrap !== null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Bootstrap already registered.", node.from.key)
            return Status.IGNORE;
        };

        const deploymentEntry = deploymentEntryUtils.encode(op.bdo.tx, requesterAddressBuffer, this.#config.addressPrefix);
        if (deploymentEntry.length === 0) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Invalid deployment entry.", node.from.key)
            return Status.FAILURE;
        };

        const feeAmount = toBalance(transactionUtils.FEE);
        if (feeAmount === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Invalid fee amount.", node.from.key)
            return Status.FAILURE;
        };

        // charge fee from the invoker
        const requesterNodeEntryBuffer = await this.#getEntryApply(requesterAddressString, batch);
        if (requesterNodeEntryBuffer === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Invalid requester node entry buffer.", node.from.key)
            return Status.FAILURE;
        };

        const requesterNodeEntry = nodeEntryUtils.decode(requesterNodeEntryBuffer, this.#config.addressPrefix);
        if (requesterNodeEntry === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Invalid requester node entry.", node.from.key)
            return Status.FAILURE;
        };

        const requesterBalance = toBalance(requesterNodeEntry.balance);
        if (requesterBalance === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Invalid requester balance.", node.from.key)
            return Status.FAILURE;
        };

        if (!requesterBalance.greaterThanOrEquals(feeAmount)) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Insufficient requester balance.", node.from.key)
            return Status.IGNORE;
        };

        const newRequesterBalance = requesterBalance.sub(feeAmount);
        if (newRequesterBalance === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Failed to apply fee to requester.", node.from.key)
            return Status.FAILURE;
        };

        const updatedRequesterNodeEntry = newRequesterBalance.update(requesterNodeEntryBuffer);
        if (updatedRequesterNodeEntry === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Failed to update requester node balance.", node.from.key)
            return Status.FAILURE;
        };

        // reward validator for processing this transaction.
        const validatorNodeEntry = nodeEntryUtils.decode(validatorEntryBuffer, this.#config.addressPrefix);
        if (validatorNodeEntry === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Invalid validator node entry.", node.from.key)
            return Status.FAILURE;
        };

        const validatorBalance = toBalance(validatorNodeEntry.balance);
        if (validatorBalance === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Invalid validator balance.", node.from.key)
            return Status.FAILURE;
        };

        const newValidatorBalance = validatorBalance.add(feeAmount.percentage(PERCENT_75));
        if (newValidatorBalance === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Failed to transfer fee to validator.", node.from.key)
            return Status.FAILURE;
        };

        const updatedValidatorNodeEntry = newValidatorBalance.update(validatorEntryBuffer);
        if (updatedValidatorNodeEntry === null) {
            this.#safeLogApply(OperationType.BOOTSTRAP_DEPLOYMENT, "Failed to update validator node balance.", node.from.key)
            return Status.FAILURE;
        };

        await batch.put(EntryType.DEPLOYMENT + bootstrapDeploymentHexString, deploymentEntry);
        await batch.put(requesterAddressString, updatedRequesterNodeEntry);
        await batch.put(validatorAddressString, updatedValidatorNodeEntry);
        await batch.put(hashHexString, node.value);

        if (this.#config.enableTxApplyLogs) {
            console.info(`Deployment operation: ${hashHexString} and deployment/${bootstrapDeploymentHexString} have been appended.`);
        }
        return Status.SUCCESS;
    }

    async #handleApplyTxOperation(op, view, base, node, batch) {
        // ATTENTION: The sanitization should be done before ANY other check, otherwise we risk crashing
        if (!this.check.validateTransactionOperation(op)) {
            this.#safeLogApply(OperationType.TX, "Contract schema validation failed.", node.from.key)
            return Status.FAILURE;
        };
        // reject transaction which is not complete
        if (!Object.hasOwn(op.txo, "vs") || !Object.hasOwn(op.txo, "va") || !Object.hasOwn(op.txo, "vn")) {
            this.#safeLogApply(OperationType.TX, "Operation is not complete.", node.from.key)
            return Status.FAILURE;
        };
        // reject if the validator signed their own transaction
        if (b4a.equals(op.address, op.txo.va)) {
            this.#safeLogApply(OperationType.TX, "Validator cannot sign its own transaction.", node.from.key)
            return Status.FAILURE;
        };
        // reject if the nonces are the same
        if (b4a.equals(op.txo.in, op.txo.vn)) {
            this.#safeLogApply(OperationType.TX, "Nonces should not be the same.", node.from.key)
            return Status.FAILURE;
        };
        // reject if the signatures are the same
        if (b4a.equals(op.txo.is, op.txo.vs)) {
            this.#safeLogApply(OperationType.TX, "Signatures should not be the same.", node.from.key)
            return Status.FAILURE;
        };
        // reject if the external bootstrap is the same as the network bootstrap
        if (b4a.equals(op.txo.bs, op.txo.mbs)) {
            this.#safeLogApply(OperationType.TX, "Network and external bootstrap cannot be the same.", node.from.key)
            return Status.FAILURE;
        };

        if (!b4a.equals(op.txo.mbs, this.#config.bootstrap)) {
            this.#safeLogApply(OperationType.TX, "Declared MSB bootstrap is different than real MSB bootstrap.", node.from.key)
            return Status.FAILURE;
        };

        // validate invoker signature
        const requesterAddressBuffer = op.address;
        const requesterAddressString = addressUtils.bufferToAddress(requesterAddressBuffer, this.#config.addressPrefix);
        if (requesterAddressString === null) {
            this.#safeLogApply(OperationType.TX, "Invalid requester address.", node.from.key)
            return Status.FAILURE;
        };

        const requesterPublicKey = PeerWallet.decodeBech32mSafe(requesterAddressString);
        if (requesterPublicKey === null) {
            this.#safeLogApply(OperationType.TX, "Failed to decode requester public key.", node.from.key)
            return Status.FAILURE;
        };

        const requesterMessage = createMessage(
            this.#config.networkId,
            op.txo.txv,
            op.txo.iw,
            op.txo.ch,
            op.txo.bs,
            this.#config.bootstrap,
            op.txo.in,
            OperationType.TX
        );
        if (requesterMessage.length === 0) {
            this.#safeLogApply(OperationType.TX, "Invalid requester message.", node.from.key)
            return Status.FAILURE;
        };

        const regeneratedTxHash = await PeerWallet.blake3Safe(requesterMessage);
        if (!b4a.equals(regeneratedTxHash, op.txo.tx)) {
            this.#safeLogApply(OperationType.TX, "Message hash does not match the tx_hash.", node.from.key)
            return Status.FAILURE;
        };

        const isRequesterSignatureValid = this.#wallet.verify(op.txo.is, op.txo.tx, requesterPublicKey); // tx contains already a nonce.
        if (!isRequesterSignatureValid) {
            this.#safeLogApply(OperationType.TX, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        };

        //second signature
        const validatorAddressBuffer = op.txo.va;
        const validatorAddressString = addressUtils.bufferToAddress(validatorAddressBuffer, this.#config.addressPrefix);
        if (validatorAddressString === null) {
            this.#safeLogApply(OperationType.TX, "Invalid validator address.", node.from.key)
            return Status.FAILURE;
        };

        const validatorPublicKey = PeerWallet.decodeBech32mSafe(validatorAddressString);
        if (validatorPublicKey === null) {
            this.#safeLogApply(OperationType.TX, "Failed to decode validator public key.", node.from.key)
            return Status.FAILURE;
        };

        // recreate validator message
        const validatorMessage = createMessage(
            this.#config.networkId,
            op.txo.tx,
            op.txo.vn,
            OperationType.TX
        );

        if (validatorMessage.length === 0) {
            this.#safeLogApply(OperationType.TX, "Invalid validator message.", node.from.key)
            return Status.FAILURE;
        };

        const validatorMessageHash = await PeerWallet.blake3Safe(validatorMessage);
        const isValidatorSignatureValid = this.#wallet.verify(op.txo.vs, validatorMessageHash, validatorPublicKey);
        if (!isValidatorSignatureValid) {
            this.#safeLogApply(OperationType.TX, "Failed to verify validator message signature.", node.from.key)
            return Status.FAILURE;
        };

        // verify tx validity - prevent deferred execution attack
        const indexersSequenceState = await this.#getIndexerSequenceStateApply(base);
        if (indexersSequenceState === null) {
            this.#safeLogApply(OperationType.TX, "Indexer sequence state is invalid.", node.from.key)
            return Status.FAILURE;
        };

        if (!b4a.equals(op.txo.txv, indexersSequenceState)) {
            this.#safeLogApply(OperationType.TX, "Transaction was not executed.", node.from.key)
            return Status.FAILURE;
        };

        const validatorEntryBuffer = await this.#getEntryApply(validatorAddressString, batch);

        // Validator consistency checks
        const isValidatorValid = await this.#isValidatorValidApply(validatorEntryBuffer, node, op);
        if (!isValidatorValid) {
            this.#safeLogApply(OperationType.TX, "Validator consistency check failed.", node.from.key)
            return Status.FAILURE;
        }

        // anti-replay attack
        const hashHexString = op.txo.tx.toString('hex');
        const opEntry = await this.#getEntryApply(hashHexString, batch);
        if (opEntry !== null) {
            this.#safeLogApply(OperationType.TX, "Operation has already been applied.", node.from.key)
            return Status.IGNORE;
        };

        // if user is performing a transaction on non-deployed bootstrap, then we need to reject it.
        // if deployment/<bootstrap> is not null then it means that the bootstrap is already deployed, and it should
        // point to payload, which is pointing to the txHash.
        const bootstrapHasBeenRegistered = await this.#getDeploymentEntryApply(op.txo.bs.toString('hex'), batch);
        if (bootstrapHasBeenRegistered === null) {
            this.#safeLogApply(OperationType.TX, "Bootstrap has not been registered.", node.from.key)
            return Status.FAILURE;
        };

        // check the subnetwork creator address
        const deploymentEntry = deploymentEntryUtils.decode(bootstrapHasBeenRegistered, this.#config.addressLength);
        if (deploymentEntry === null) {
            this.#safeLogApply(OperationType.TX, "Invalid deployment entry.", node.from.key)
            return Status.FAILURE;
        };

        const subnetworkCreatorAddressString = addressUtils.bufferToAddress(deploymentEntry.address, this.#config.addressPrefix);
        if (subnetworkCreatorAddressString === null) {
            this.#safeLogApply(OperationType.TX, "Invalid subnet creator address.", node.from.key)
            return Status.FAILURE;
        };

        const feeAmount = toBalance(transactionUtils.FEE);
        if (feeAmount === null) {
            this.#safeLogApply(OperationType.TX, "Invalid fee amount.", node.from.key)
            return Status.FAILURE;
        };

        const transferFeeTxOperationResult = await this.#transferFeeTxOperation(
            requesterAddressString,
            validatorAddressString,
            validatorEntryBuffer,
            subnetworkCreatorAddressString,
            feeAmount,
            batch,
            node
        );
        
        // TODO: cover next 4 guards below with tests
        if (transferFeeTxOperationResult === null) {
            this.#safeLogApply(OperationType.TX, "Fee transfer operation failed completely.", node.from.key);
            return Status.FAILURE;
        }

        if (transferFeeTxOperationResult === Status.IGNORE) {
            this.#safeLogApply(OperationType.TX, "Fee transfer operation skipped.", node.from.key);
            return Status.IGNORE;
        }

        if (transferFeeTxOperationResult.requesterEntry === null) {
            this.#safeLogApply(OperationType.TX, "Failed to process requester fee deduction.", node.from.key)
            return Status.FAILURE;
        }

        if (transferFeeTxOperationResult.validatorEntry === null) {
            this.#safeLogApply(OperationType.TX, "Failed to process validator fee reward.", node.from.key)
            return Status.FAILURE;
        }

        await batch.put(requesterAddressString, transferFeeTxOperationResult.requesterEntry);
        await batch.put(validatorAddressString, transferFeeTxOperationResult.validatorEntry);

        // Handle optional subnetwork creator fee - may be null if creator is requester or validator
        if (transferFeeTxOperationResult.subnetworkCreatorEntry !== null) {
            await batch.put(subnetworkCreatorAddressString, transferFeeTxOperationResult.subnetworkCreatorEntry);
        }
        await batch.put(hashHexString, node.value);

        if (this.#config.enableTxApplyLogs) {
            console.info(`Subnetwork TX operation: ${hashHexString} has been appended.`);
        }
        return Status.SUCCESS;
    }

    async #handleApplyTransferOperation(op, view, base, node, batch) {
        if (!this.check.validateTransferOperation(op)) {
            this.#safeLogApply(OperationType.TRANSFER, "Contract schema validation failed.", node.from.key)
            return Status.FAILURE;
        };
        // if transaction is not complete, do not process it.
        if (!Object.hasOwn(op.tro, "vs") || !Object.hasOwn(op.tro, "va") || !Object.hasOwn(op.tro, "vn")) {
            this.#safeLogApply(OperationType.TRANSFER, "Operation is not complete.", node.from.key)
            return Status.FAILURE;
        };
        // for additional security, nonces should be different.
        if (b4a.equals(op.tro.in, op.tro.vn)) {
            this.#safeLogApply(OperationType.TRANSFER, "Nonces should not be the same.", node.from.key)
            return Status.FAILURE;
        };
        // addresses should be different.
        if (b4a.equals(op.address, op.tro.va)) {
            this.#safeLogApply(OperationType.TRANSFER, "Addresses should not be the same.", node.from.key)
            return Status.FAILURE;
        };
        // signatures should be different.
        if (b4a.equals(op.tro.is, op.tro.vs)) {
            this.#safeLogApply(OperationType.TRANSFER, "Signatures should not be the same.", node.from.key)
            return Status.FAILURE;
        };

        // validate requester signature
        const requesterAddressBuffer = op.address;
        const requesterAddressString = addressUtils.bufferToAddress(requesterAddressBuffer, this.#config.addressPrefix);
        if (requesterAddressString === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Requester address is invalid.", node.from.key)
            return Status.FAILURE;
        };

        const requesterPublicKey = PeerWallet.decodeBech32mSafe(requesterAddressString);
        if (requesterPublicKey === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Error while decoding requester public key.", node.from.key)
            return Status.FAILURE;
        };

        // recreate requester message
        const requesterMessage = createMessage(
            this.#config.networkId,
            op.tro.txv,
            op.tro.to,
            op.tro.am,
            op.tro.in,
            OperationType.TRANSFER
        );

        if (requesterMessage.length === 0) {
            this.#safeLogApply(OperationType.TRANSFER, "Invalid requester message.", node.from.key)
            return Status.FAILURE;
        };

        // ensure that tx is valid
        const regeneratedTxHash = await PeerWallet.blake3Safe(requesterMessage);
        if (!b4a.equals(regeneratedTxHash, op.tro.tx)) {
            this.#safeLogApply(OperationType.TRANSFER, "Message hash does not match the tx_hash.", node.from.key)
            return Status.FAILURE;
        };

        const isRequesterSignatureValid = this.#wallet.verify(op.tro.is, regeneratedTxHash, requesterPublicKey);
        if (!isRequesterSignatureValid) {
            this.#safeLogApply(OperationType.TRANSFER, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        };

        // signature of the validator
        const validatorAddressBuffer = op.tro.va;
        const validatorAddressString = addressUtils.bufferToAddress(validatorAddressBuffer, this.#config.addressPrefix);
        if (validatorAddressString === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Validator address is invalid.", node.from.key)
            return Status.FAILURE;
        };

        const validatorPublicKey = PeerWallet.decodeBech32mSafe(validatorAddressString);
        if (validatorPublicKey === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Failed to decode validator public key.", node.from.key)
            return Status.FAILURE;
        };

        const validatorMessage = createMessage(
            this.#config.networkId,
            op.tro.tx,
            op.tro.vn,
            OperationType.TRANSFER
        );

        if (validatorMessage.length === 0) {
            this.#safeLogApply(OperationType.TRANSFER, "Invalid validator message.", node.from.key)
            return Status.FAILURE;
        };

        const validatorMessageHash = await PeerWallet.blake3Safe(validatorMessage);
        const isValidatorSignatureValid = this.#wallet.verify(op.tro.vs, validatorMessageHash, validatorPublicKey);
        if (!isValidatorSignatureValid) {
            this.#safeLogApply(OperationType.TRANSFER, "Failed to verify message signature.", node.from.key)
            return Status.FAILURE;
        };

        // verify tx validity - prevent deferred execution attack
        const indexersSequenceState = await this.#getIndexerSequenceStateApply(base);
        if (indexersSequenceState === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Indexer sequence state is invalid.", node.from.key)
            return Status.FAILURE;
        };

        if (!b4a.equals(op.tro.txv, indexersSequenceState)) {
            this.#safeLogApply(OperationType.TRANSFER, "Transaction was not executed.", node.from.key)
            return Status.FAILURE;
        };

        const validatorEntryBuffer = await this.#getEntryApply(validatorAddressString, batch);

        // Validator consistency checks
        const isValidatorValid = await this.#isValidatorValidApply(validatorEntryBuffer, node, op);
        if (!isValidatorValid) {
            this.#safeLogApply(OperationType.TRANSFER, "Validator consistency check failed.", node.from.key)
            return Status.FAILURE;
        }

        // anti-replay attack
        const hashHexString = op.tro.tx.toString('hex');
        const opEntry = await this.#getEntryApply(hashHexString, batch);
        if (opEntry !== null) {
            this.#safeLogApply(OperationType.TRANSFER, "Operation has already been applied.", node.from.key)
            return Status.IGNORE;
        };

        // Check if recipient address is valid.
        const recipientAddressBuffer = op.tro.to;
        const recipientAddressString = addressUtils.bufferToAddress(recipientAddressBuffer, this.#config.addressPrefix);
        if (recipientAddressString === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Invalid recipient address.", node.from.key)
            return Status.FAILURE;
        };

        const recipientPublicKey = PeerWallet.decodeBech32mSafe(recipientAddressString);
        if (recipientPublicKey === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Failed to decode recipient public key.", node.from.key)
            return Status.FAILURE;
        };

        const isSelfTransfer = b4a.equals(requesterAddressBuffer, recipientAddressBuffer);
        const isRecipientValidator = b4a.equals(recipientAddressBuffer, validatorAddressBuffer);

        const transferResult = await this.#transfer(
            requesterAddressString,
            recipientAddressString,
            validatorAddressString,
            validatorEntryBuffer,
            op.tro.am,
            transactionUtils.FEE,
            isSelfTransfer,
            isRecipientValidator,
            batch,
            node
        );

        if (transferResult === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Invalid transfer result.", node.from.key);
            return Status.FAILURE;
        }

        if (transferResult === Status.IGNORE) {
            this.#safeLogApply(OperationType.TRANSFER, "Transfer operation skipped.", node.from.key);
            return Status.IGNORE;
        };

        if (transferResult.senderEntry === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Invalid sender entry.", node.from.key)
            return Status.FAILURE;
        };

        if (transferResult.validatorEntry === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Invalid validator entry.", node.from.key)
            return Status.FAILURE;
        };

        if (!isSelfTransfer) {
            if (transferResult.recipientEntry === null) {
                this.#safeLogApply(OperationType.TRANSFER, "Invalid recipient entry.", node.from.key)
                return Status.FAILURE;
            };

            await batch.put(recipientAddressString, transferResult.recipientEntry);
        }

        await batch.put(requesterAddressString, transferResult.senderEntry);
        await batch.put(validatorAddressString, transferResult.validatorEntry);

        if (!isSelfTransfer && !isRecipientValidator && transferResult.recipientEntry !== null) {
            await batch.put(recipientAddressString, transferResult.recipientEntry);
        }

        await batch.put(hashHexString, node.value);

        if (this.#config.enableTxApplyLogs) {
            console.info(`Transfer operation: ${hashHexString} has been appended.`);
        }
        return Status.SUCCESS;
    }

    async #transfer(senderAddressString, recipientAddressString, validatorAddressString, validatorEntryBuffer, transferAmountBuffer, feeAmountBuffer, isSelfTransfer, isRecipientValidator, batch, node) {
        if (!senderAddressString ||
            !recipientAddressString ||
            !validatorAddressString ||
            !validatorEntryBuffer ||
            !transferAmountBuffer ||
            !feeAmountBuffer ||
            (isSelfTransfer === null || isSelfTransfer === undefined) ||
            !batch ||
            !node
        ) {
            this.#safeLogApply(OperationType.TRANSFER, "Invalid transfer incoming data.", node.from.key)
            return null;
        }

        const transferAmount = toBalance(transferAmountBuffer);
        const feeAmount = toBalance(feeAmountBuffer);
        if (transferAmount === null || feeAmount === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Invalid fee/transfer amount.", node.from.key)
            return null;
        }

        // totalDeductedAmount = transferAmount + fee. When transferamount is 0, then totalDeductedAmount = fee. Because 0 + fee = fee.
        const totalDeductedAmount = isSelfTransfer ? feeAmount : transferAmount.add(feeAmount);
        if (totalDeductedAmount === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Invalid total deducted amount.", node.from.key)
            return null;
        }

        const senderEntryBuffer = await this.#getEntryApply(senderAddressString, batch);
        if (senderEntryBuffer === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Invalid sender node entry buffer.", node.from.key)
            return null;
        }

        const senderEntry = nodeEntryUtils.decode(senderEntryBuffer, this.#config.addressPrefix);
        if (senderEntry === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Invalid sender node entry.", node.from.key)
            return null;
        }

        const senderBalance = toBalance(senderEntry.balance);
        if (senderBalance === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Invalid sender balance.", node.from.key)
            return null;
        }

        if (!senderBalance.greaterThanOrEquals(totalDeductedAmount)) {
            this.#safeLogApply(OperationType.TRANSFER, "Insufficient sender balance.", node.from.key)
            return Status.IGNORE;
        }

        const newSenderBalance = senderBalance.sub(totalDeductedAmount);
        if (newSenderBalance === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Failed to apply fee to sender node balance.", node.from.key)
            return null;
        }

        const updatedSenderEntry = newSenderBalance.update(senderEntryBuffer);
        if (updatedSenderEntry === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Failed to update sender node balance.", node.from.key)
            return null;
        }

        const result = {
            senderEntry: updatedSenderEntry,
            recipientEntry: null,
            validatorEntry: null,
        };

        if (!isSelfTransfer && !isRecipientValidator) {
            const recipientEntryBuffer = await this.#getEntryApply(recipientAddressString, batch);
            if (recipientEntryBuffer === null) {
                if (transferAmount.value === null) {
                    this.#safeLogApply(OperationType.TRANSFER, "Invalid transfer amount.", node.from.key)
                    return null;
                };
                const newRecipientEntry = nodeEntryUtils.init(
                    ZERO_WK,
                    nodeRoleUtils.NodeRole.READER,
                    transferAmount.value
                );
                if (newRecipientEntry.length === 0) {
                    this.#safeLogApply(OperationType.TRANSFER, "Invalid recipient entry.", node.from.key)
                    return null;
                };
                result.recipientEntry = newRecipientEntry;
            } else {
                const recipientEntry = nodeEntryUtils.decode(recipientEntryBuffer, this.#config.addressPrefix);
                if (recipientEntry === null) {
                    this.#safeLogApply(OperationType.TRANSFER, "Invalid recipient entry.", node.from.key)
                    return null;
                };

                const recipientBalance = toBalance(recipientEntry.balance);
                if (recipientBalance === null) {
                    this.#safeLogApply(OperationType.TRANSFER, "Invalid recipient balance.", node.from.key)
                    return null;
                };

                const newRecipientBalance = recipientBalance.add(transferAmount);
                if (newRecipientBalance === null) {
                    this.#safeLogApply(OperationType.TRANSFER, "Failed to transfer amount to recipient balance.", node.from.key)
                    return null;
                };

                const updatedRecipientEntry = newRecipientBalance.update(recipientEntryBuffer);
                if (updatedRecipientEntry === null) {
                    this.#safeLogApply(OperationType.TRANSFER, "Failed to update recipient node balance.", node.from.key)
                    return null;
                };
                result.recipientEntry = updatedRecipientEntry;
            }
        }

        const validatorEntry = nodeEntryUtils.decode(validatorEntryBuffer, this.#config.addressPrefix);
        if (validatorEntry === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Invalid validator entry.", node.from.key)
            return null;
        }

        const validatorBalance = toBalance(validatorEntry.balance);
        if (validatorBalance === null) return null;

        const validatorReward = feeAmount.percentage(PERCENT_75);
        if (validatorReward === null) return null;

        const newValidatorBalance = isRecipientValidator
            ? validatorBalance.add(transferAmount).add(validatorReward)
            : validatorBalance.add(validatorReward);

        if (newValidatorBalance === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Failed to transfer fee to validator balance.", node.from.key)
            return null;
        }

        const updatedValidatorEntry = newValidatorBalance.update(validatorEntryBuffer);
        if (updatedValidatorEntry === null) {
            this.#safeLogApply(OperationType.TRANSFER, "Failed to update validator node balance.", node.from.key)
            return null;
        }

        result.validatorEntry = updatedValidatorEntry;

        if (isRecipientValidator) {
            result.recipientEntry = updatedValidatorEntry;
        }

        return result;
    }

    #isAdminApply(adminEntry, node) {
        if (!adminEntry || !node) return false;
        return b4a.equals(adminEntry.wk, node.from.key);
    }

    async #getEntryApply(key, batch) {
        const entry = await batch.get(key);
        return deepCopyBuffer(entry?.value)
    }

    async #getDeploymentEntryApply(key, batch) {
        const entry = await batch.get(EntryType.DEPLOYMENT + key);
        return deepCopyBuffer(entry?.value)
    }

    async #getIndexerSequenceStateApply(base) {
        try {
            const buf = [];
            for (const indexer of Object.values(base.system.indexers)) {
                buf.push(indexer.key);
            }
            return await PeerWallet.blake3Safe(b4a.concat(buf));
        } catch (error) {
            console.error(error);
            return null;
        }
    }

    async #isWriterKeyInIndexerListApply(wk, base) {
        try {
            return Object.values(base.system.indexers).some(entry => b4a.equals(entry.key, wk));
        } catch (error) {
            console.error(error);
            return null
        }
    }
    async #isValidatorValidApply(validatorEntryBuffer, node, op) {
        // TODO: Maybe we should transfer all validator checks to this function (address, pubKey, signature, etc)
        if (validatorEntryBuffer === null) {
            this.#safeLogApply(op.type, "Incoming validator entry is null.", node.from.key)
            return false;
        };

        const decodedValidatorEntry = nodeEntryUtils.decode(validatorEntryBuffer, this.#config.addressPrefix);
        if (decodedValidatorEntry === null) {
            this.#safeLogApply(op.type, "Failed to decode validator entry.", node.from.key)
            return false;
        };

        // validator must be active writer
        if (!decodedValidatorEntry.isWriter) {
            this.#safeLogApply(op.type, "Operation validator is not active", node.from.key)
            return false;
        };

        // The autobase payload should be appended by the node that signed the partial operation.
        const validatorWk = decodedValidatorEntry.wk;
        if (!b4a.equals(validatorWk, node.from.key)) {
            this.#safeLogApply(op.type, "Validator cannot be the same as requester.", node.from.key)
            return false;
        }
        return true;
    }

    /**
     * Retrieves the address assigned to a given writing key from the registry.
     * 
     * @param {Object} batch - The current Hyperbee batch instance used for reading state.
     * @param {string} writingKey - The writing key in hex string format.
     * @returns {Buffer|null} The address buffer assigned to the writing key, or null if not registered.
     */
    async #getRegisteredWriterKeyApply(batch, writingKey) {
        const entry = await batch.get(EntryType.WRITER_ADDRESS + writingKey);
        return deepCopyBuffer(entry?.value)
    }

    async #isApplyInitalizationDisabled(batch) {
        // Retrieve the flag to verify if initialization is allowed
        let initialization = await this.#getEntryApply(EntryType.INITIALIZATION, batch);
        if (initialization === null) {
            return false
        } else {
            return b4a.equals(initialization, safeWriteUInt32BE(0, 0))
        }
    }

    async #updateWritersIndex(batch) {
        // Retrieve and increment the writers length entry
        let length = await this.#getEntryApply(EntryType.WRITERS_LENGTH, batch);
        let incrementedLength = null;
        if (length === null) {
            // Initialize the writers length entry if it does not exist
            const bufferedLength = lengthEntryUtils.init(0);
            length = lengthEntryUtils.decodeBE(bufferedLength);
            incrementedLength = lengthEntryUtils.incrementBE(length);
        } else {
            // Decode and increment the existing writers length entry
            length = lengthEntryUtils.decodeBE(length);
            incrementedLength = lengthEntryUtils.incrementBE(length);
        }

        return { length, incrementedLength }
    }

    #safeLogApply(operationType = "Common", errorMessage, writingKey = null) {
        if (!this.#config.enableErrorApplyLogs) return;
        try {
            const date = new Date().toISOString();
            const wk = writingKey ? writingKey.toString('hex') : 'N/A';
            console.error(`[${date}][${operationType}][${errorMessage}][${wk}]`);
        } catch (e) {
            console.error(`[LOG_ERROR][Failed to log error][${e}]`);
        }
    }

    #stakeBalanceApply(nodeEntryBuffer, node) {
        if (!nodeEntryBuffer || nodeEntryBuffer.length === 0 || nodeEntryBuffer.length !== NODE_ENTRY_SIZE) {
            this.#safeLogApply("StakeBalance", "Invalid node entry buffer", node.from.key);
            return null;
        }

        const decodedNodeEntry = nodeEntryUtils.decode(nodeEntryBuffer, this.#config.addressPrefix);
        if (decodedNodeEntry === null) {
            this.#safeLogApply("StakeBalance", "Failed to decode node entry", node.from.key);
            return null;
        }

        const currentNodeBalance = toBalance(decodedNodeEntry.balance);
        if (currentNodeBalance === null) {
            this.#safeLogApply("StakeBalance", "Invalid node balance", node.from.key);
            return null;
        }

        if (!currentNodeBalance.greaterThanOrEquals(BALANCE_TO_STAKE)) {
            this.#safeLogApply("StakeBalance", "Insufficient balance to stake", node.from.key);
            return null;
        }

        const newNodeBalance = currentNodeBalance.sub(BALANCE_TO_STAKE);
        if (newNodeBalance === null) {
            this.#safeLogApply("StakeBalance", "Failed to subtract stake balance", node.from.key);
            return null;
        }

        const updatedNodeEntryWithBalance = newNodeBalance.update(nodeEntryBuffer);
        if (updatedNodeEntryWithBalance === null) {
            this.#safeLogApply("StakeBalance", "Failed to update node entry with new balance", node.from.key);
            return null;
        }

        const updatedNodeEntryWithAllBalances = nodeEntryUtils.setStakedBalance(updatedNodeEntryWithBalance, BALANCE_TO_STAKE.value);
        if (updatedNodeEntryWithAllBalances === null) {
            this.#safeLogApply("StakeBalance", "Failed to set staked balance in node entry", node.from.key);
            return null;
        }

        return updatedNodeEntryWithAllBalances;
    }

    #withdrawStakedBalanceApply(nodeEntryBuffer, node) {
        if (!nodeEntryBuffer || nodeEntryBuffer.length === 0 || nodeEntryBuffer.length !== NODE_ENTRY_SIZE) {
            this.#safeLogApply("withdrawStakedBalanceApply", "Invalid node entry buffer", node.from.key);
            return null;
        }

        const decodedNodeEntry = nodeEntryUtils.decode(nodeEntryBuffer, this.#config.addressPrefix);
        if (decodedNodeEntry === null) {
            this.#safeLogApply("withdrawStakedBalanceApply", "Failed to decode node entry", node.from.key);
            return null;
        }

        const stakedBalance = toBalance(decodedNodeEntry.stakedBalance);
        if (stakedBalance === null) {
            this.#safeLogApply("withdrawStakedBalanceApply", "Invalid staked balance", node.from.key);
            return null;
        }

        if (!stakedBalance.greaterThan(BALANCE_ZERO)) {
            this.#safeLogApply("withdrawStakedBalanceApply", "No staked balance to unstake", node.from.key);
            return null;
        }

        const currentNodeBalance = toBalance(decodedNodeEntry.balance);
        if (currentNodeBalance === null) {
            this.#safeLogApply("withdrawStakedBalanceApply", "Invalid current balance", node.from.key);
            return null;
        }

        const newNodeBalance = currentNodeBalance.add(stakedBalance);
        if (newNodeBalance === null) {
            this.#safeLogApply("withdrawStakedBalanceApply", "Failed to add staked balance to current balance", node.from.key);
            return null;
        }

        const updatedNodeEntryWithBalance = newNodeBalance.update(nodeEntryBuffer);
        if (updatedNodeEntryWithBalance === null) {
            this.#safeLogApply("withdrawStakedBalanceApply", "Failed to update node entry with new balance", node.from.key);
            return null;
        }

        const updatedNodeEntryWithAllBalances = nodeEntryUtils.setStakedBalance(updatedNodeEntryWithBalance, BALANCE_ZERO.value);
        if (updatedNodeEntryWithAllBalances === null) {
            this.#safeLogApply("withdrawStakedBalanceApply", "Failed to set staked balance in node entry", node.from.key);
            return null;
        }

        return updatedNodeEntryWithAllBalances;

    }

    async #validatorPenaltyApply(writingKeyBuffer, batch, base, invalidOperations) {
        const adminEntryBuffer = await this.#getEntryApply(EntryType.ADMIN, batch);
        if (adminEntryBuffer === null) {
            this.#safeLogApply("ValidatorPenalty", "Admin entry not found", writingKeyBuffer);
            return;
        }
        const adminEntry = adminEntryUtils.decode(adminEntryBuffer, this.#config.addressPrefix);
        if (adminEntry === null) {
            this.#safeLogApply("ValidatorPenalty", "Failed to decode admin entry", writingKeyBuffer);
            return;
        }

        if (b4a.equals(adminEntry.wk, writingKeyBuffer)) {
            this.#safeLogApply("ValidatorPenalty", "Admin cannot be penalized", writingKeyBuffer);
            return;
        }

        // In theory, none of the negative cases in the if-statements should occur. They are added only for safety reasons.
        const validatorWk = writingKeyBuffer.toString('hex');

        const validatorAddressBuffer = await this.#getRegisteredWriterKeyApply(batch, validatorWk);
        if (validatorAddressBuffer === null) {
            this.#safeLogApply("ValidatorPenalty", `No validator found for writing key: ${validatorWk}`, writingKeyBuffer);
            return;
        }

        const validatorAddressString = addressUtils.bufferToAddress(validatorAddressBuffer, this.#config.addressPrefix);
        if (validatorAddressString === null) {
            this.#safeLogApply("ValidatorPenalty", `Invalid validator address: ${validatorAddressString}`, writingKeyBuffer);
            return;
        }

        const validatorPublicKey = PeerWallet.decodeBech32mSafe(validatorAddressString);
        if (validatorPublicKey === null) {
            this.#safeLogApply("ValidatorPenalty", `Failed to decode validator public key: ${validatorAddressString}`, writingKeyBuffer);
            return;
        }

        const validatorNodeEntryBuffer = await this.#getEntryApply(validatorAddressString, batch);
        if (validatorNodeEntryBuffer === null) {
            this.#safeLogApply("ValidatorPenalty", `No node entry found for validator address: ${validatorAddressString}`, writingKeyBuffer);
            return;
        }

        const decodedValidatorNodeEntry = nodeEntryUtils.decode(validatorNodeEntryBuffer, this.#config.addressPrefix);
        if (decodedValidatorNodeEntry === null) {
            this.#safeLogApply("ValidatorPenalty", `Failed to decode validator node entry for address: ${validatorAddressString}`, writingKeyBuffer);
            return;
        }

        const stakedBalance = toBalance(decodedValidatorNodeEntry.stakedBalance);

        if (stakedBalance === null) {
            this.#safeLogApply("ValidatorPenalty", `Invalid staked balance for validator address: ${validatorAddressString}`, writingKeyBuffer);
            return;
        }

        const penalty = BALANCE_FEE.mul(toTerm(BigInt(invalidOperations)));

        if (penalty === null) {
            this.#safeLogApply("ValidatorPenalty", `Failed to calculate penalty for validator address: ${validatorAddressString}`, writingKeyBuffer);
            return;
        }

        const deductedStakedBalance = penalty.greaterThanOrEquals(stakedBalance) ? BALANCE_ZERO : stakedBalance.sub(penalty);

        if (deductedStakedBalance === null) {
            this.#safeLogApply("ValidatorPenalty", `Failed to subtract penalty from staked balance for validator address: ${validatorAddressString}`, writingKeyBuffer);
            return;
        }

        if (deductedStakedBalance.greaterThan(BALANCE_ZERO)) {

            const currentBalance = toBalance(decodedValidatorNodeEntry.balance);
            if (currentBalance === null) {
                this.#safeLogApply("ValidatorPenalty", `Invalid balance for validator address: ${validatorAddressString}`, writingKeyBuffer);
                return;
            }

            const newBalance = currentBalance.add(deductedStakedBalance);
            if (newBalance === null) {
                this.#safeLogApply("ValidatorPenalty", `Failed to add remaining staked balance to balance for validator address: ${validatorAddressString}`, writingKeyBuffer);
                return;
            }

            const updatedNodeEntryWithBalance = newBalance.update(validatorNodeEntryBuffer);
            if (updatedNodeEntryWithBalance === null) {
                this.#safeLogApply("ValidatorPenalty", `Failed to update node entry with new balance for validator address: ${validatorAddressString}`, writingKeyBuffer);
                return;
            }

            const updatedNodeEntryWithAllBalances = nodeEntryUtils.setStakedBalance(updatedNodeEntryWithBalance, BALANCE_ZERO.value);
            if (updatedNodeEntryWithAllBalances === null) {
                this.#safeLogApply("ValidatorPenalty", `Failed to update node entry with new staked balance for validator address: ${validatorAddressString}`, writingKeyBuffer);
                return;
            }

            const downgradedNodeEntry = nodeEntryUtils.setRole(updatedNodeEntryWithAllBalances, nodeRoleUtils.NodeRole.WHITELISTED);

            if (downgradedNodeEntry === null) {
                this.#safeLogApply("ValidatorPenalty", `Failed to downgrade validator to whitelisted for address: ${validatorAddressString}`, writingKeyBuffer);
                return;
            }

            await base.removeWriter(writingKeyBuffer);
            await batch.put(validatorAddressString, downgradedNodeEntry);

            return;

        } else {
            const updatedNodeEntryZeroStakedBalance = nodeEntryUtils.setStakedBalance(validatorNodeEntryBuffer, BALANCE_ZERO.value);
            if (updatedNodeEntryZeroStakedBalance === null) {
                this.#safeLogApply("ValidatorPenalty", `Failed to update node entry with new staked balance for validator address: ${validatorAddressString}`, writingKeyBuffer);
                return;
            }

            const downgradedNodeEntry = nodeEntryUtils.setRole(updatedNodeEntryZeroStakedBalance, nodeRoleUtils.NodeRole.WHITELISTED);
            if (downgradedNodeEntry === null) {
                this.#safeLogApply("ValidatorPenalty", `Failed to downgrade validator to whitelisted for address: ${validatorAddressString}`, writingKeyBuffer);
                return;
            }

            await base.removeWriter(writingKeyBuffer);
            await batch.put(validatorAddressString, downgradedNodeEntry);

            return;
        }
    }

    async #applyGetLicenseCount(batch) {
        return await this.#getEntryApply(EntryType.LICENSE_COUNT, batch)
    }

    async #applyAssignNewLicense(batch) {
        let licenseCount = await this.#applyGetLicenseCount(batch)
        let newLicenseLength;
        if (licenseCount === null) {
            // Initialize the writers length entry if it does not exist
            const bufferedLength = lengthEntryUtils.init(0);
            licenseCount = lengthEntryUtils.decodeBE(bufferedLength);
            newLicenseLength = lengthEntryUtils.incrementBE(licenseCount);
        } else {
            // Decode and increment the existing writers length entry
            licenseCount = lengthEntryUtils.decodeBE(licenseCount);
            newLicenseLength = lengthEntryUtils.incrementBE(licenseCount);
        }
        const decodedNewLicenseLength = lengthEntryUtils.decodeBE(newLicenseLength);

        return { newLicenseLength, decodedNewLicenseLength };
    }

    #emitEvent(event, ...args) {
        try {
            this.emit(event, ...args)
        } catch (_ignored) { }
    }

    async #transferFeeTxOperation(requesterAddressString, validatorAddressString, validatorEntryBuffer, subnetworkCreatorAddressString, feeAmount, batch, node) {
        if (!requesterAddressString ||
            !validatorAddressString ||
            !validatorEntryBuffer ||
            !subnetworkCreatorAddressString ||
            !feeAmount ||
            !batch ||
            !node
        ) {
            this.#safeLogApply("transferFeeTxOperation", "Invalid incoming data.", node.from.key)
            return null;
        }

        // case when requester is also the validator is not possible.
        // charge fee from the requester
        const requesterNodeEntryBuffer = await this.#getEntryApply(requesterAddressString, batch);
        if (requesterNodeEntryBuffer === null) {
            this.#safeLogApply("transferFeeTxOperation", "Invalid requester node entry buffer.", node.from.key)
            return null;
        }

        const requesterNodeEntry = nodeEntryUtils.decode(requesterNodeEntryBuffer, this.#config.addressPrefix);
        if (requesterNodeEntry === null) {
            this.#safeLogApply("transferFeeTxOperation", "Invalid requester node entry, can not to decode.", node.from.key)
            return null;
        }

        const requesterBalance = toBalance(requesterNodeEntry.balance);
        if (requesterBalance === null) {
            this.#safeLogApply("transferFeeTxOperation", "Invalid requester balance.", node.from.key)
            return null;
        }

        if (!requesterBalance.greaterThanOrEquals(feeAmount)) {
            this.#safeLogApply("transferFeeTxOperation", "Insufficient requester balance to pay fee.", node.from.key)
            return Status.IGNORE;
        }

        const newRequesterBalance = requesterBalance.sub(feeAmount);
        if (newRequesterBalance === null) {
            this.#safeLogApply("transferFeeTxOperation", "Failed to deduct fee from requester balance.", node.from.key)
            return null;
        }

        const updatedRequesterNodeEntry = newRequesterBalance.update(requesterNodeEntryBuffer);
        if (updatedRequesterNodeEntry === null) {
            this.#safeLogApply("transferFeeTxOperation", "Failed to update requester node balance.", node.from.key)
            return null;
        }

        // Validator always gets 50% of the fee by the base

        const validatorNodeEntry = nodeEntryUtils.decode(validatorEntryBuffer, this.#config.addressPrefix);
        if (validatorNodeEntry === null) {
            this.#safeLogApply("transferFeeTxOperation", "Invalid validator node entry, can not to decode.", node.from.key)
            return null;
        }

        const validatorBalance = toBalance(validatorNodeEntry.balance);
        if (validatorBalance === null) {
            this.#safeLogApply("transferFeeTxOperation", "Invalid validator balance.", node.from.key)
            return null;
        }

        const newValidatorBalance = validatorBalance.add(feeAmount.percentage(PERCENT_50));
        if (newValidatorBalance === null) {
            this.#safeLogApply("transferFeeTxOperation", "Failed to add fee to validator balance.", node.from.key)
            return null;
        }

        const updatedValidatorNodeEntry = newValidatorBalance.update(validatorEntryBuffer);
        if (updatedValidatorNodeEntry === null) {
            this.#safeLogApply("transferFeeTxOperation", "Failed to update validator node balance.", node.from.key)
            return null;
        }

        // If requester is the subnetwork creator:
        // 1. Validator got 50%
        // 2. Other 50% is burned
        // 3. No additional reward for bootstrap deployer
        if (requesterAddressString === subnetworkCreatorAddressString) {
            return {
                requesterEntry: updatedRequesterNodeEntry,
                validatorEntry: updatedValidatorNodeEntry,
                subnetworkCreatorEntry: null
            };
        }

        // If validator is also the bootstrap deployer:
        // 1. Gets 75% total (50% as validator + 25% as bootstrap deployer)
        // 2. 25% is burned
        if (validatorAddressString === subnetworkCreatorAddressString) {
            // We already added 50% as validator fee, now add only the additional 25%
            const newValidatorBalanceWithBonus = newValidatorBalance.add(feeAmount.percentage(PERCENT_25));
            if (newValidatorBalanceWithBonus === null) {
                this.#safeLogApply("transferFeeTxOperation", "Failed to add bonus fee to validator balance.", node.from.key)
                return null;
            }

            const updatedValidatorNodeEntryWithBonus = newValidatorBalanceWithBonus.update(validatorEntryBuffer);
            if (updatedValidatorNodeEntryWithBonus === null) {
                this.#safeLogApply("transferFeeTxOperation", "Failed to update validator node balance with bonus.", node.from.key)
                return null;
            }

            return {
                requesterEntry: updatedRequesterNodeEntry,
                validatorEntry: updatedValidatorNodeEntryWithBonus,
                subnetworkCreatorEntry: null
            };
        }

        // Normal case (validator is not the bootstrap deployer):
        // 1. Validator got 50%
        // 2. Bootstrap deployer gets 25%
        // 3. 25% is burned
        const subnetworkCreatorNodeEntryBuffer = await this.#getEntryApply(subnetworkCreatorAddressString, batch);
        if (subnetworkCreatorNodeEntryBuffer === null) {
            this.#safeLogApply("transferFeeTxOperation", "Invalid subnetwork creator -  it does not exists", node.from.key)
            return null;
        }

        const subnetworkCreatorNodeEntry = nodeEntryUtils.decode(subnetworkCreatorNodeEntryBuffer, this.#config.addressPrefix);
        if (subnetworkCreatorNodeEntry === null) {
            this.#safeLogApply("transferFeeTxOperation", "Invalid subnetwork creator node entry, can not to decode.", node.from.key)
            return null;
        }

        const subnetworkCreatorBalance = toBalance(subnetworkCreatorNodeEntry.balance);
        if (subnetworkCreatorBalance === null) {
            this.#safeLogApply("transferFeeTxOperation", "Invalid subnetwork creator balance.", node.from.key)
            return null;
        }

        const newSubnetworkCreatorBalance = subnetworkCreatorBalance.add(feeAmount.percentage(PERCENT_25));
        if (newSubnetworkCreatorBalance === null) {
            this.#safeLogApply("transferFeeTxOperation", "Failed to add fee to subnetwork creator balance.", node.from.key)
            return null;
        }

        const updatedSubnetworkCreatorNodeEntry = newSubnetworkCreatorBalance.update(subnetworkCreatorNodeEntryBuffer);
        if (updatedSubnetworkCreatorNodeEntry === null) {
            this.#safeLogApply("transferFeeTxOperation", "Failed to update subnetwork creator node balance.", node.from.key)
            return null;
        }

        return {
            requesterEntry: updatedRequesterNodeEntry,
            validatorEntry: updatedValidatorNodeEntry,
            subnetworkCreatorEntry: updatedSubnetworkCreatorNodeEntry
        };
    }

}

export default State;
