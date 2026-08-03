import ReadyResource from "ready-resource";
import Corestore from "corestore";
import Rache from "rache";
import tracCryptoApi from "trac-crypto-api";
import b4a from "b4a";
import { sleep, isHexString } from "./utils/helpers.js";
import { applyStateMessageFactory } from "./messages/state/applyStateMessageFactory.js";
import { isAddressValid } from "./core/state/utils/address.js";
import Network from "./core/network/Network.js";
import Check from "./utils/check.js";
import State from "./core/state/State.js";
import {
    EventType,
    WHITELIST_SLEEP_INTERVAL,
    BOOTSTRAP_HEXSTRING_LENGTH,
    CustomEventType,
    BALANCE_MIGRATION_SLEEP_INTERVAL,
    WHITELIST_MIGRATION_DIR,
    OperationType
} from "./utils/constants.js";
import { decimalStringToBigInt, bigIntTo16ByteBuffer, bufferToBigInt, bigIntToDecimalString } from "./utils/amountSerialization.js"
import {
    normalizeDecodedPayloadForJson,
    normalizeTransactionOperation,
    normalizeTransferOperation
} from "./utils/normalizers.js";
import fileUtils from './utils/fileUtils.js';
import migrationUtils from './utils/migrationUtils.js';
import {safeDecodeApplyOperation, safeEncodeApplyOperation} from "./utils/protobuf/operationHelpers.js";
import PartialTransactionValidator from "./core/network/protocols/shared/validators/PartialTransactionValidator.js";
import PartialTransferValidator from "./core/network/protocols/shared/validators/PartialTransferValidator.js";
import { BroadcastError, ValidationError } from "./utils/errors.js";

export class MainSettlementBus extends ReadyResource {
    #store;
    #wallet;
    #network;
    #state;
    #isClosing = false;
    #config

    /**
     * @param {import("./config/config.js").Config} config
     * @param {import("trac-wallet").Wallet | undefined} wallet
     **/
    constructor(config, wallet = undefined) {
        super();
        this.#config = config
        this.#wallet = wallet;
        this.#store = new Corestore(this.#config.storesFullPath, {
            globalCache: new Rache({ maxSize: this.#config.hyperbeeCacheMaxEntries })
        });
        this.check = new Check(this.#config);
    }

    get config() {
        return this.#config
    }

    get state() {
        return this.#state;
    }

    get network() {
        return this.#network;
    }

    get wallet() {
        if (!this.#config.enableWallet) {
            return undefined;
        }
        return this.#wallet;
    }

    async _open() {
        this.#state = new State(this.#store, this.#wallet, this.#config);
        this.#network = new Network(this.#state, this.#config, this.#wallet?.address ?? null);

        await this.#state.ready();
        await this.#network.ready();
        await this.#stateEventsListener();

        if (this.#wallet) {
            this.#printWalletInfo();
        }

        await this.#network.replicate(
            this.#state,
            this.#store,
            this.#wallet,
        );

        const adminEntry = await this.#state.getAdminEntry();
        await this.#setUpRoleAutomatically(adminEntry);



        console.log(`isIndexer: ${this.#state.isIndexer()}`);
        console.log(`isWriter: ${this.#state.isWritable()}`);
        console.log("MSB Unsigned Length:", this.#state.getUnsignedLength());
        console.log("MSB Signed Length:", this.#state.getSignedLength());

        if (this.#wallet) {
            await this.#printBalance();
        }
    }

    async _close() {
        console.log("Closing everything gracefully... This may take a moment.");

        this.#isClosing = true;
        await this.#network.close();

        await sleep(100);

        await this.#state.close();

        await sleep(100);

        if (this.#store !== null) {
            await this.#store.close();
        }

        await sleep(100);
    }

    async destroy() {
        return await this.close();
    }

    async broadcastPartialTransaction(partialTransactionPayload) {
        return await this.#network.validatorMessageOrchestrator.send(partialTransactionPayload);
    }

    async validateTransfer(payload) {
        const partialTransferValidator = new PartialTransferValidator(this.#state, null, this.#config);
        return await partialTransferValidator.validate(payload);
    }

    // Used by peer simulation
    async validateTransaction(payload) {
        const partialTransactionValidator = new PartialTransactionValidator(this.#state, null, this.#config);
        return await partialTransactionValidator.validate(payload);
    }

    async broadcastTransaction(payload) {
        if (!payload) {
            throw new ValidationError("Transaction payload is required for broadcasting.");
        }

        let normalizedPayload;
        let hash;

        if (payload.type === OperationType.TRANSFER) {
            normalizedPayload = normalizeTransferOperation(payload, this.#config);
            await this.validateTransfer(normalizedPayload);
            hash = b4a.toString(normalizedPayload.tro.tx, "hex");
        } else if (payload.type === OperationType.TX) {
            normalizedPayload = normalizeTransactionOperation(payload, this.#config);
            try {
                await this.validateTransaction(normalizedPayload);
            } catch {
                // We swap exceptions to keep compatibility
                throw new ValidationError("Invalid transaction payload.");
            }
            hash = b4a.toString(normalizedPayload.txo.tx, "hex");
        }

        const success = await this.broadcastPartialTransaction(payload);
        if (!success) {
            throw new BroadcastError("Failed to broadcast transaction after multiple attempts.");
        }

        const isConfirmed = await this.#state.waitForUnsigned(
            hash,
            this.#config.messageValidatorResponseTimeout,
            100
        );
        if (!isConfirmed) {
            throw new BroadcastError("Failed to broadcast transaction after multiple attempts.");
        }

        return {
            signedLength: this.#state.getSignedLength(),
            unsignedLength: this.#state.getUnsignedLength(),
            tx: hash
        };
    }

    async #setUpRoleAutomatically() {
        if (!this.#state.isWritable() && this.#config.enableRoleRequester) {
            setTimeout(async () => {
                await this.requestWriterRole(true);
            }, 5_000);
            await sleep(5_000);
        }
    }

    isAdmin(adminEntry) {
        if (!adminEntry || !this.#config.enableWallet) return false;
        return this.#wallet.address === adminEntry.address && b4a.equals(adminEntry.wk, this.#state.writingKey)
    }

    async #isAllowedToRequestRole(adminEntry, nodeEntry) {
        return nodeEntry?.isWhitelisted && !this.isAdmin(adminEntry);
    }

    async #stateEventsListener() {
        this.#state.on(CustomEventType.IS_INDEXER, (publicKey) => {
            if (this.#isClosing) return;
            this.#network.disconnectValidatorPeer(publicKey, 'peer promoted to indexer');
        });

        this.#state.on(CustomEventType.UNWRITABLE, (publicKey) => {
            if (this.#isClosing) return;
            this.#network.disconnectValidatorPeer(publicKey, 'peer became unwritable');
        });

        this.#state.base.on(EventType.IS_INDEXER, () => {
            console.log("Current node is an indexer");
        });

        this.#state.base.on(EventType.IS_NON_INDEXER, async () => {
            // Prevent further actions if closing is in progress
            // The reason is that getNodeEntry is async and may cause issues if we will access state after closing
            if (this.#isClosing) return;
            console.log("Current node is not an indexer anymore");
        });

        this.#state.base.on(EventType.WRITABLE, async () => {
            console.log("Current node is writable");
        });

        this.#state.base.on(EventType.UNWRITABLE, async () => {
            console.log("Current node is unwritable");
        });
    }

    async getConfirmedTxInfo(txHash) {
        const payload = await this.#state.getSigned(txHash);
        if (!payload) return null

        const decoded = safeDecodeApplyOperation(payload);
        if (!decoded) {
            throw new Error(`Failed to decode payload for transaction hash: ${txHash}`);
        }

        return { payload, decoded }
    }

    async getUnconfirmedTxInfo(txHash) {
        const payload = await this.#state.get(txHash);
        if (!payload) return null

        const decoded = safeDecodeApplyOperation(payload);
        if (!decoded) {
            throw new Error(`Failed to decode payload for transaction hash: ${txHash}`);
        }

        return { payload, decoded }
    }

    handleGetFee() {        
        const fee = this.#state.getFee();
        return bufferToBigInt(fee);
    }

    async getBalance(address, confirmed = true) {
        const nodeEntry = confirmed
            ? await this.#state.getNodeEntry(address)
            : await this.#state.getNodeEntryUnsigned(address);

        if (!nodeEntry) {
            return undefined;
        }

        return {
            address,
            balance: bufferToBigInt(nodeEntry.balance).toString(),
        };
    }

    async getTxv() {
        const txv = await this.#state.getIndexerSequenceState();
        return txv.toString("hex");
    }

    getConfirmedLength() {
        return this.#state.getSignedLength();
    }

    getUnconfirmedLength() {
        return this.#state.getUnsignedLength();
    }

    async verifyDag() {
        try {
            let dagView = await this.#state.base.view.core.treeHash();
            let lengthdagView = this.#state.base.view.core.length;
            let dagSystem = await this.#state.base.system.core.treeHash();
            let lengthdagSystem = this.#state.base.system.core.length;
            const wl = await this.#state.getWriterLength();
            
            console.log("---------- node & network stats ----------");
            console.log("wallet.publicKey:", this.#wallet?.publicKey?.toString("hex") ?? "unset");
            console.log("wallet.address:", this.#wallet?.address ?? "unset");
            console.log("msb.writerKey:", this.#state?.writingKey ? this.#state.writingKey.toString("hex") : "unset");
            console.log("swarm.connections.size:", this.#network?.swarm?.connections?.size || 0);
            console.log("base.view.core.signedLength:", this.#state.base.view.core.signedLength ?? "unset");
            console.log("base.view.core.length:", this.#state.base.view.core.length ?? "unset");
            console.log("base.signedLength", this.#state.base.signedLength ?? "unset");
            console.log("base.indexedLength", this.#state.base.indexedLength ?? "unset");
            console.log("base.linearizer.indexers.length", this.#state.base.linearizer?.indexers?.length ?? "unset");
            console.log(`base.key: ${this.#state.base.key ? this.#state.base.key.toString("hex") : "unset"}`);
            console.log("discoveryKey:", this.#state.base.discoveryKey ? b4a.toString(this.#state.base.discoveryKey, "hex") : "unset");
            console.log(`VIEW Dag: ${dagView ? dagView.toString("hex") : "unset"} (length: ${lengthdagView || 0})`);
            console.log(`SYSTEM Dag: ${dagSystem ? dagSystem.toString("hex") : "unset"} (length: ${lengthdagSystem || 0})`);
            console.log("Total Registered Writers:", wl !== null ? wl : 0);
            console.log("---------- flags ----------");
            console.log(`isIndexer: ${this.#state?.isIndexer?.() ?? "unset"}`);
            console.log(`isWriter: ${this.#state?.isWritable?.() ?? "unset"}`);
        } catch (error) {
            console.error("Error during DAG monitoring:", error.message);
        }
    }

    printHelp() {
        if (this.#config.isAdminMode) {
            console.log("🚨 WARNING: IF YOU ARE NOT AN ADMIN, DO NOT USE THE COMMANDS BELOW! YOU RISK LOSING YOUR FUNDS! 🚨");
            console.log("\nAdmin commands:");
            console.log("- /add_admin: Register admin entry with bootstrap key (initial setup), or use --recovery flag to recover admin role");
            console.log("- /balance_migration: Perform balance migration with the given initial balances CSV file");
            console.log("- /add_whitelist: Add all specified whitelist addresses. If initialization is enabled, no fee is required.");
            console.log("- /disable_initialization: Disable further balance initializations and whitelisting");
            console.log("- /add_indexer <address>: Change a role of the selected writer node to indexer role. Charges a fee.");
            console.log("- /remove_indexer <address>: Change a role of the selected indexer node to default role. Charges a fee.");
            console.log("- /ban_writer <address>: Demote a whitelisted writer to default role and remove it from the whitelist. Charges a fee.");
        }
        console.log("Available commands:");
        console.log("- /add_writer: Add yourself as a validator to this MSB once whitelisted. Requires a fee + 10x the fee as a stake in $TNK.");
        console.log("- /remove_writer: Remove yourself from this MSB. Requires a fee, and the stake will be refunded.");
        console.log("- /node_status <address>: Get network information about a node with the given address.");
        console.log("- /stats: Check system stats such as writing key, DAG, etc.");
        console.log("- /deployment <subnetwork_bootstrap> <channel>: Deploy a subnetwork with the given bootstrap. If channel is not provided, a random one will be generated. Requires a fee.");
        console.log("- /get_deployment <subnetwork_bootstrap>: Get information about a subnetwork deployment with the given bootstrap.");
        console.log("- /transfer <to_address> <amount>: Transfer the specified amount to the given address. Requires a fee.");
        console.log("- /get_tx_info <tx_hash>: Get information about a transaction with the given hash.");
        console.log("- /get_validator_addr <writing_key>: Get the validator address mapped to the given writing key.");
        console.log("- /get_balance <address> <confirmed>: Get the balance of the node with specified address (confirmed = true is default)");
        console.log("- /exit: Exit the program.");
        console.log("- /help: Display this help.");
    }

    #printWalletInfo() {
        console.log("");
        console.log("#####################################################################################");
        console.log("# MSB Address:   ", this.#wallet.address, " #");
        console.log("# MSB Writer:    ", this.#state.writingKey.toString("hex"), "#");
        console.log("#####################################################################################");
    }

    async #printBalance() {
        const nodeEntry = await this.#state.getNodeEntry(this.#wallet.address);
        const balance = nodeEntry ? bigIntToDecimalString(bufferToBigInt(nodeEntry.balance)) : "0";
        console.log(`Balance: ${balance}`);
    }

    async getTxHashes(start, end) {
        const hashes = await this.#state.confirmedTransactionsBetween(start, end);
        return { hashes };
    }

    async getTxDetails(hash) {
        const rawPayload = await this.getConfirmedTxInfo(hash);
        if (!rawPayload) {
            return null;
        }

        return normalizeDecodedPayloadForJson(rawPayload.decoded, this.#config);
    }

    async fetchBulkTxPayloads(hashes) {
        const response = { results: [], missing: [] };
        const results = await Promise.all(hashes.map((hash) => this.getConfirmedTxInfo(hash)));

        results.forEach((result, index) => {
            const hash = hashes[index];
            if (!result) {
                response.missing.push(hash);
                return;
            }

            const decodedResult = normalizeDecodedPayloadForJson(result.decoded, this.#config);
            response.results.push({ hash, payload: decodedResult });
        });

        return response;
    }

    async getExtendedTxDetails(hash, confirmed) {
        const rawPayload = confirmed
            ? await this.getConfirmedTxInfo(hash)
            : await this.getUnconfirmedTxInfo(hash);

        if (!rawPayload) {
            return null;
        }

        const txDetails = normalizeDecodedPayloadForJson(rawPayload.decoded, this.#config);
        const confirmedLength = await this.#state.getTransactionConfirmedLength(hash);

        if (confirmedLength === null) {
            return {
                txDetails,
                confirmed_length: 0,
                fee: "0",
            };
        }

        return {
            txDetails,
            confirmed_length: confirmedLength,
            fee: this.handleGetFee().toString(),
        };
    }

    async handleAdminCreation() {
        if (!this.#config.enableWallet) {
            throw new Error("Can not initialize an admin - wallet is not enabled.");
        }

        const adminEntry = await this.#state.getAdminEntry();

        if (adminEntry) {
            throw new Error("Can not initialize an admin - admin already exists.");
        }
        if (!this.#wallet) {
            throw new Error(
                "Can not initialize an admin - wallet is not initialized."
            );
        }
        if (!this.#state.writingKey) {
            throw new Error(
                "Can not initialize an admin - writing key is not initialized."
            );
        }
        if (!b4a.equals(this.#state.writingKey, this.#config.bootstrap)) {
            throw new Error(
                "Can not initialize an admin - bootstrap is not equal to writing key."
            );
        }

        const txValidity = await tracCryptoApi.hash.blake3(this.#config.bootstrap);
        const addAdminMessage = await applyStateMessageFactory(this.#wallet, this.#config)
            .buildCompleteAddAdminMessage(
                this.#wallet.address,
                this.#state.writingKey,
                txValidity,
            )
        const encodedPayload = safeEncodeApplyOperation(addAdminMessage);

        await this.#state.append(encodedPayload);
    }

    async handleAdminRecovery() {
        if (!this.#config.enableWallet) {
            throw new Error("Can not initialize an admin - wallet is not enabled.");
        }

        const adminEntry = await this.#state.getAdminEntry();

        if (!adminEntry) {
            throw new Error(
                "Can not perform recovery - admin has been not initialized."
            );
        }
        if (!this.#wallet) {
            throw new Error("Can not perform recovery - wallet is not initialized.");
        }
        if (adminEntry.address !== this.#wallet.address) {
            throw new Error("Can not perform recovery - you are not the admin.");
        }
        if (b4a.equals(this.#state.writingKey, adminEntry.wk)) {
            throw new Error(
                "Can not perform recovery - writer key condition is not met."
            );
        }

        const txValidity = await this.#state.getIndexerSequenceState();
        const adminRecoveryMessage = await applyStateMessageFactory(this.#wallet, this.#config)
            .buildPartialAdminRecoveryMessage(
                this.#wallet.address,
                this.#state.writingKey,
                txValidity,
                "json"
            )

        const success = await this.broadcastPartialTransaction(adminRecoveryMessage);

        if (!success) {
            throw new Error("Failed to broadcast transaction. Try again later.");
        }

        console.info(`Transaction hash: ${adminRecoveryMessage.rao.tx}`);
    }

    async handleWhitelistOperations() {
        if (!this.#config.enableWallet) {
            throw new Error("Cannot perform whitelisting - wallet is not enabled.");
        }

        const adminEntry = await this.#state.getAdminEntry();

        if (!this.isAdmin(adminEntry)) {
            throw new Error('Cannot perform whitelisting - you are not the admin!.');
        }

        const messages = new Map();
        const addresses = await fileUtils.readAddressesFromWhitelistFile();

        for (const address of addresses) {
            await migrationUtils.validateAddressFromIncomingFile(this.#state, this.#config, address, adminEntry);
        }
        await fileUtils.validateWhitelistMigrationData(addresses, WHITELIST_MIGRATION_DIR);
        const migrationNumber = await fileUtils.getNextMigrationNumber(WHITELIST_MIGRATION_DIR);
        await fileUtils.createWhitelistEntryFile(addresses, migrationNumber, WHITELIST_MIGRATION_DIR);

        for (const addressToWhitelist of addresses) {
            const txValidity = await this.#state.getIndexerSequenceState();
            const appendWhitelistMessage = await applyStateMessageFactory(this.#wallet, this.#config)
                .buildCompleteAppendWhitelistMessage(
                    this.#wallet.address,
                    addressToWhitelist,
                    txValidity,
                )
            const encodedPayload = safeEncodeApplyOperation(appendWhitelistMessage)
            messages.set(addressToWhitelist, encodedPayload);
        }

        if (!messages || messages.size === 0) {
            throw new Error("No whitelisted messages to process.");
        }

        const totalElements = messages.size;
        let processedCount = 0;

        for (const [address, encodedPayload] of messages) {
            processedCount++;
            const isWhitelisted = await this.#state.isAddressWhitelisted(address);
            if (isWhitelisted) {
                console.error(`Public key ${address} is already whitelisted.`);
                console.log(
                    `Whitelist message skipped (${processedCount}/${totalElements})`
                );
                continue;
            }

            await this.#state.append(encodedPayload);
            // timesleep and validate if it becomes whitelisted
            // if node is not active we should not wait to long...

            await sleep(WHITELIST_SLEEP_INTERVAL);
            console.log(
                `Whitelist message processed (${processedCount}/${totalElements})`
            );
        }
    }

    async requestWriterRole(toAdd) {
        if (!this.#config.enableWallet) {
            throw new Error("Cannot request writer role - wallet is not enabled");
        }

        const adminEntry = await this.#state.getAdminEntry();
        const nodeEntry = await this.#state.getNodeEntry(this.#wallet.address);
        const isAlreadyWriter = !!nodeEntry?.isWriter;

        if (toAdd) {
            if (isAlreadyWriter) {
                throw new Error(
                    "Cannot request writer role - state indicates you are already a writer"
                );
            }

            const isAllowedToRequestRole = await this.#isAllowedToRequestRole(
                adminEntry,
                nodeEntry
            );

            if (!isAllowedToRequestRole) {
                throw new Error(
                    "Cannot request writer role - node is not allowed to request this role"
                );
            }

            if (this.#state.isWritable()) {
                throw new Error(
                    "Cannot request writer role - internal state is already writable"
                );
            }

            const requiredBalance = bufferToBigInt(this.#state.getFee()) * 11n;
            const nodeBalance = bufferToBigInt(nodeEntry.balance);

            if (nodeBalance < requiredBalance) {
                throw new Error(`Cannot add writer role - insufficient balance. Required: ${bigIntToDecimalString(requiredBalance)}, Available: ${bigIntToDecimalString(nodeBalance)}`);
            }

            const txValidity = await this.#state.getIndexerSequenceState();

            const assembledMessage = await applyStateMessageFactory(this.#wallet, this.#config)
                .buildPartialAddWriterMessage(
                    this.#wallet.address,
                    this.#state.writingKey,
                    txValidity,
                    'json'
                )

            const success = await this.broadcastPartialTransaction(assembledMessage);

            if (!success) {
                throw new Error("Failed to broadcast transaction. Try again later.");
            }

            console.info(`Transaction hash: ${assembledMessage.rao.tx}`);
            return;
        }

        if (!isAlreadyWriter) {
            throw new Error("Cannot remove writer role - you are not a writer");
        }

        if (nodeEntry.isIndexer === true) {
            throw new Error("Cannot remove writer role - node is an indexer");
        }

        const requiredBalance = bufferToBigInt(this.#state.getFee());
        const nodeBalance = bufferToBigInt(nodeEntry.balance);

        if (nodeBalance < requiredBalance) {
            throw new Error(`Cannot remove writer role - insufficient balance. Required: ${bigIntToDecimalString(requiredBalance)}, Available: ${bigIntToDecimalString(nodeBalance)}`);
        }

        const txValidity = await this.#state.getIndexerSequenceState();
        const assembledMessage = await applyStateMessageFactory(this.#wallet, this.#config)
            .buildPartialRemoveWriterMessage(
                this.#wallet.address,
                nodeEntry.wk,
                txValidity,
                "json"
            )

        const success = await this.broadcastPartialTransaction(assembledMessage);

        if (!success) {
            throw new Error("Failed to broadcast transaction. Try again later.");
        }

        console.info(`Transaction hash: ${assembledMessage.rao.tx}`);
    }

    async updateWriterToIndexerRole(addressToUpdate, toAdd) {
        if (!this.#config.enableWallet) {
            throw new Error(
                `Can not request indexer role for: ${addressToUpdate} - wallet is not enabled.`
            );
        }

        const adminEntry = await this.#state.getAdminEntry();

        if (!adminEntry) {
            throw new Error(
                `Can not request indexer role for: ${addressToUpdate} - admin entry has not been initialized.`
            );
        }

        if (!isAddressValid(addressToUpdate, this.#config.addressPrefix)) {
            throw new Error(
                `Can not request indexer role for: ${addressToUpdate} - invalid address.`
            );
        }

        if (!this.isAdmin(adminEntry) && !this.#state.isWritable()) {
            throw new Error(
                `Can not request indexer role for: ${addressToUpdate} - You are not an admin or writer.`
            );
        }
        const nodeEntry = await this.#state.getNodeEntry(addressToUpdate);
        if (!nodeEntry) {
            throw new Error(
                `Can not request indexer role for: ${addressToUpdate} - node entry has not been not initialized.`
            );
        }

        const indexerNodeEntry = await this.#state.getNodeEntry(addressToUpdate);
        const indexerListHasAddress = await this.#state.isWkInIndexersEntry(
            indexerNodeEntry.wk,
        );

        if (toAdd) {
            if (indexerListHasAddress) {
                throw new Error(
                    `Cannot update indexer role for: ${addressToUpdate} - address is already in indexers list.`
                );
            }

            const canAddIndexer =
                nodeEntry.isWhitelisted &&
                nodeEntry.isWriter &&
                !nodeEntry.isIndexer &&
                !indexerListHasAddress;

            if (!canAddIndexer) {
                throw new Error(
                    `Can not request indexer role for: ${addressToUpdate} - node is not whitelisted, not a writer or already an indexer.`
                );
            }
            const txValidity = await this.#state.getIndexerSequenceState();

            const assembledAddIndexerMessage = await applyStateMessageFactory(this.#wallet, this.#config)
                .buildCompleteAddIndexerMessage(
                    this.#wallet.address,
                    addressToUpdate,
                    txValidity,
                )

            const encodedPayload = safeEncodeApplyOperation(assembledAddIndexerMessage);

            await this.#state.append(encodedPayload);
        } else {
            const canRemoveIndexer =
                !toAdd && nodeEntry.isIndexer && indexerListHasAddress;

            if (!canRemoveIndexer) {
                throw new Error(
                    `Can not remove indexer role for: ${addressToUpdate} - node is not an indexer or address is not in indexers list.`
                );
            }
            const txValidity = await this.#state.getIndexerSequenceState();
            const assembledRemoveIndexerMessage = await applyStateMessageFactory(this.#wallet, this.#config)
                .buildCompleteRemoveIndexerMessage(
                    this.#wallet.address,
                    addressToUpdate,
                    txValidity,
                )
            const encodedPayload = safeEncodeApplyOperation(assembledRemoveIndexerMessage);

            await this.#state.append(encodedPayload);
        }
    }

    async banValidator(addresstToBan) {
        if (!this.#config.enableWallet) {
            throw new Error(
                `Can not ban writer with address: ${addresstToBan} - wallet is not enabled.`
            );
        }
        const adminEntry = await this.#state.getAdminEntry();

        if (!adminEntry) {
            throw new Error(
                `Can not ban writer with address: ${addresstToBan} - admin entry has not been initialized.`
            );
        }

        if (!isAddressValid(addresstToBan, this.#config.addressPrefix)) {
            throw new Error(
                `Can not ban writer with address:  ${addresstToBan} - invalid address.`
            );
        }

        if (!this.isAdmin(adminEntry)) {
            throw new Error(
                `Can not ban writer with address: ${addresstToBan} - You are not an admin.`
            );
        }

        const isWhitelisted = await this.#state.isAddressWhitelisted(addresstToBan);
        const nodeEntry = await this.#state.getNodeEntry(addresstToBan);

        if (!isWhitelisted || null === nodeEntry || nodeEntry.isIndexer === true) {
            throw new Error(
                `Can not ban writer with address: ${addresstToBan} - node is not whitelisted or is an indexer.`
            );
        }
        const txValidity = await this.#state.getIndexerSequenceState();
        const assembledBanValidatorMessage = await applyStateMessageFactory(this.#wallet, this.#config)
            .buildCompleteBanWriterMessage(
                this.#wallet.address,
                addresstToBan,
                txValidity,
            )
        const encodedPayload = safeEncodeApplyOperation(assembledBanValidatorMessage)
        await this.#state.append(encodedPayload);
    }

    async deployBootstrap(externalBootstrap, channel) {
        if (!this.#config.enableWallet) {
            throw new Error(
                "Can not perform bootstrap deployment - wallet is not enabled."
            );
        }

        if (!this.#wallet) {
            throw new Error(
                "Can not perform bootstrap deployment - wallet is not initialized."
            );
        }

        const adminEntry = await this.#state.getAdminEntry();

        if (!adminEntry) {
            throw new Error(
                `Can not perform bootstrap deployment - admin entry has not been initialized.`
            );
        }

        if (!externalBootstrap) {
            throw new Error(
                `Can not perform bootstrap deployment - external bootstrap is not provided.`
            );
        }

        if (!channel) {
            throw new Error(
                `Can not perform bootstrap deployment - channel is not provided.`
            );
        }

        if (channel.length !== 64 || !isHexString(channel)) {
            throw new Error(`Can not perform bootstrap deployment - channel is not a hex: ${channel}`);
        }

        if (externalBootstrap.length !== BOOTSTRAP_HEXSTRING_LENGTH || !isHexString(externalBootstrap)) {
            throw new Error(
                `Can not perform bootstrap deployment - bootstrap is not a hex: ${externalBootstrap}`
            );
        }
        const isAlreadyDeployed = await this.#state.getRegisteredBootstrapEntry(externalBootstrap);
        if (isAlreadyDeployed !== null) {
            throw new Error(
                `Can not perform bootstrap deployment - bootstrap ${externalBootstrap} is already deployed.`
            );
        }

        if (externalBootstrap === this.#config.bootstrap.toString("hex")) {
            throw new Error(
                `Can not perform bootstrap deployment - bootstrap ${externalBootstrap} is equal to MSB bootstrap!`
            );
        }

        // Check if we have enough balance for the fee
        const senderEntry = await this.#state.getNodeEntry(this.#wallet.address);
        if (!senderEntry) {
            throw new Error("Sender account not found");
        }

        const fee = this.#state.getFee();
        const feeBigInt = bufferToBigInt(fee);
        const senderBalance = bufferToBigInt(senderEntry.balance);

        if (senderBalance < feeBigInt) {
            throw new Error(`Insufficient balance to cover deployment fee. Required: ${bigIntToDecimalString(feeBigInt)}, Available: ${bigIntToDecimalString(senderBalance)}`);
        }

        const txValidity = await this.#state.getIndexerSequenceState();

        const payload = await applyStateMessageFactory(this.#wallet, this.#config)
            .buildPartialBootstrapDeploymentMessage(
                this.#wallet.address,
                externalBootstrap,
                channel,
                txValidity,
                "json"
            )

        const success = await this.broadcastPartialTransaction(payload);

        if (!success) {
            throw new Error("Failed to broadcast transaction. Try again later.");
        }

        console.info(`Transaction hash: ${payload.bdo.tx}`);
        console.log(`Bootstrap ${externalBootstrap} deployment requested on channel ${channel}`);
        console.log('Bootstrap Deployment Fee Details:');
        console.log(`Fee: ${bigIntToDecimalString(feeBigInt)}`);
        console.log(`Expected Balance After Deployment: ${bigIntToDecimalString(senderBalance - feeBigInt)}`);

    }

    async prepareTransferOperation(recipientAddress, amount) {
        if (!this.#config.enableWallet) {
            throw new Error(
                "Can not perform transfer - wallet is not enabled."
            );
        }

        if (!this.#wallet) {
            throw new Error(
                "Can not perform transfer - wallet is not initialized."
            );
        }

        if (!isAddressValid(recipientAddress, this.#config.addressPrefix)) {
            throw new Error("Invalid recipient address");
        }

        const amountBigInt = decimalStringToBigInt(amount);

        const MAX_AMOUNT = BigInt('0xffffffffffffffffffffffffffffffff');
        if (amountBigInt > MAX_AMOUNT) {
            throw new Error("Transfer amount exceeds maximum allowed value");
        }

        const amountBuffer = bigIntTo16ByteBuffer(amountBigInt);

        if (bufferToBigInt(amountBuffer) !== amountBigInt) {
            throw new Error(`conversion error for amount: ${amount}`);
        }

        const senderEntry = await this.#state.getNodeEntry(this.#wallet.address);
        if (!senderEntry) {
            throw new Error("Sender account not found");
        }

        const fee = this.#state.getFee();
        const feeBigInt = bufferToBigInt(fee);
        const senderBalance = bufferToBigInt(senderEntry.balance);
        const isSelfTransfer = recipientAddress === this.#wallet.address;
        const totalDeductedAmount = isSelfTransfer ? feeBigInt : amountBigInt + feeBigInt;

        if (!(senderBalance >= totalDeductedAmount)) {
            throw new Error("Insufficient balance for transfer + fee");
        }

        const txValidity = await this.#state.getIndexerSequenceState();
        const payload = await applyStateMessageFactory(this.#wallet, this.#config)
            .buildPartialTransferOperationMessage(
                this.#wallet.address,
                recipientAddress,
                amountBuffer,
                txValidity,
                "json"
            )

        const expectedNewBalance = senderBalance - totalDeductedAmount;
        return {
            payload,
            amountBigInt,
            feeBigInt,
            senderBalance,
            totalDeductedAmount,
            expectedNewBalance,
            isSelfTransfer
        };
    }

    async submitPreparedTransferOperation(preparedTransfer) {
        const success = await this.broadcastPartialTransaction(preparedTransfer.payload);
        if (!success) {
            throw new Error("Failed to broadcast transfer transaction after multiple attempts.");
        } else {
            console.log(`Transfer transaction broadcasted successfully. Tx hash: ${preparedTransfer.payload.tro.tx}`);
        }

        return preparedTransfer.payload.tro.tx;
    }

    async balanceMigrationOperation() {
        const isInitDisabled = await this.#state.isInitalizationDisabled()

        if (isInitDisabled) {
            throw new Error("Can not initialize balance - balance initialization is disabled.");
        }

        if (!this.#config.enableWallet) {
            throw new Error("Can not initialize an admin - wallet is not enabled.");
        }

        const adminEntry = await this.#state.getAdminEntry();

        if (!adminEntry) {
            throw new Error("Can not initialize an admin - admin does not exist.");
        }

        if (!this.isAdmin(adminEntry)) {
            throw new Error('Cannot perform balance migration - you are not the admin!.');
        }

        if (!this.#wallet) {
            throw new Error("Can not initialize an admin - wallet is not initialized.");
        }

        if (!this.#state.writingKey) {
            throw new Error("Can not initialize an admin - writing key is not initialized.");
        }

        if (!b4a.equals(this.#state.writingKey, this.#config.bootstrap)) {
            throw new Error("Can not initialize an admin - bootstrap is not equal to writing key.");
        }

        const { addressBalancePair, totalBalance, totalAddresses, addresses } = await fileUtils.readBalanceMigrationFile();

        for (let i = 0; i < addresses.length; i++) {
            await migrationUtils.validateAddressFromIncomingFile(this.#state, this.#config, addresses[i].address, adminEntry);
        }

        await fileUtils.validateBalanceMigrationData(addresses);
        const migrationNumber = await fileUtils.getNextMigrationNumber();
        await fileUtils.createMigrationEntryFile(addressBalancePair, migrationNumber);

        const txValidity = await this.#state.getIndexerSequenceState();

        let messages = [];

        for (const [recipientAddress, amountBuffer] of addressBalancePair) {
            const payload = await applyStateMessageFactory(this.#wallet, this.#config).buildCompleteBalanceInitializationMessage(
                this.#wallet.address,
                recipientAddress,
                amountBuffer,
                txValidity,
            )
            messages.push(safeEncodeApplyOperation(payload));
        }

        console.log(`Total balance to migrate: ${bigIntToDecimalString(totalBalance)} across ${totalAddresses} addresses.`);

        if (messages.length === 0) {
            throw new Error("No balance migration messages to process.");
        }

        console.log("Starting BRC20 $TRAC TO $TNK native migration...");
        for (let i = 0; i < messages.length; i++) {
            const message = messages[i];
            console.log(`Processing message ${i + 1} of ${messages.length}...`);
            await this.#state.append(message);
            await sleep(BALANCE_MIGRATION_SLEEP_INTERVAL);

        }

        await sleep(5000);
        let allBalancesMigrated = true;

        console.log("Verifying migrated balances for unsigned length...");
        for (let i = 0; i < addresses.length; i++) {
            const entry = await this.#state.getNodeEntryUnsigned(addresses[i].address);
            const expectedBalance = addresses[i].parsedBalance;
            const amountBigInt = bufferToBigInt(entry.balance);
            if (amountBigInt !== expectedBalance) {
                allBalancesMigrated = false
                console.log(`Balance of ${addresses[i].address} failed to migrate. Expected: ${expectedBalance}, Found: ${amountBigInt} for unsigned length`);
            }
        }

        console.log("Verifying migrated balances for signed length...");
        for (let i = 0; i < addresses.length; i++) {
            const entry = await this.#state.getNodeEntry(addresses[i].address);
            const expectedBalance = addresses[i].parsedBalance;
            const amountBigInt = bufferToBigInt(entry.balance);
            if (amountBigInt !== expectedBalance) {
                allBalancesMigrated = false
                console.log(`Balance of ${addresses[i].address} failed to migrate. Expected: ${expectedBalance}, Found: ${amountBigInt} for signed length`);
            }
        }

        console.log("Balance migration successful: ", allBalancesMigrated);
        console.log(`${bigIntToDecimalString(totalBalance)} $TNK have been migrated across ${totalAddresses} addresses.`);
    }

    async disableInitialization() {
        if (!this.#config.enableWallet) {
            throw new Error("Can not initialize an admin - wallet is not enabled.");
        }
        const isInitDisabled = await this.#state.isInitalizationDisabled();
        if (isInitDisabled) {
            throw new Error("Can not disable initialization - it is already disabled.");
        }
        const adminEntry = await this.#state.getAdminEntry();

        if (!adminEntry) {
            throw new Error("Can not initialize an admin - admin does not exist.");
        }

        if (!this.isAdmin(adminEntry)) {
            throw new Error('Cannot perform whitelisting - you are not the admin!.');
        }
        if (!this.#wallet) {
            throw new Error("Can not initialize an admin - wallet is not initialized.");
        }
        if (!this.#state.writingKey) {
            throw new Error("Can not initialize an admin - writing key is not initialized.");
        }
        const txValidity = await this.#state.getIndexerSequenceState();


        const payload = await applyStateMessageFactory(this.#wallet, this.#config)
            .buildCompleteDisableInitializationMessage(
                this.#wallet.address,
                this.#state.writingKey,
                txValidity,
            )
        console.log('Disabling initialization...');
        const encodedPayload = safeEncodeApplyOperation(payload);
        await this.#state.append(encodedPayload);
    }
}

export default MainSettlementBus;
