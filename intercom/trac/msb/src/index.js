/** @typedef {import('pear-interface')} */ /* global Pear */
import ReadyResource from "ready-resource";
import Corestore from "corestore";
import PeerWallet from "trac-wallet";
import b4a from "b4a";
import readline from "readline";
import tty from "tty";
import { sleep, isHexString } from "./utils/helpers.js";
import { verifyDag, printHelp, printWalletInfo, printBalance } from "./utils/cli.js";
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
    WHITELIST_MIGRATION_DIR
} from "./utils/constants.js";
import { randomBytes } from "hypercore-crypto";
import { decimalStringToBigInt, bigIntTo16ByteBuffer, bufferToBigInt, bigIntToDecimalString } from "./utils/amountSerialization.js"
import fileUtils from './utils/fileUtils.js';
import migrationUtils from './utils/migrationUtils.js';
import {
    getBalanceCommand,
    getTxvCommand,
    getFeeCommand,
    getConfirmedLengthCommand,
    getUnconfirmedLengthCommand,
    getTxPayloadsBulkCommand,
    getTxHashesCommand,
    getTxDetailsCommand,
    getExtendedTxDetailsCommand,
    nodeStatusCommand,
    coreInfoCommand,
    getValidatorAddressCommand,
    getDeploymentCommand,
    getTxInfoCommand,
    getLicenseNumberCommand,
    getLicenseAddressCommand,
    getLicenseCountCommand
} from "./utils/cliCommands.js";
import {safeEncodeApplyOperation} from "./utils/protobuf/operationHelpers.js";

export class MainSettlementBus extends ReadyResource {
    #store;
    #wallet;
    #network;
    #readline_instance;
    #state;
    #isClosing = false;
    #config

    /**
     * @param {object} config
     **/
    constructor(config) {
        super();
        this.#config = config
        this.#store = new Corestore(this.#config.storesFullPath);
        this.#wallet = new PeerWallet({ networkPrefix: this.#config.addressPrefix });
        this.#readline_instance = null;

        if (this.#config.enableInteractiveMode) {
            try {
                this.#readline_instance = readline.createInterface({
                    input: new tty.ReadStream(0),
                    output: new tty.WriteStream(1),
                });
            } catch (_ignored) {}
        }

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
        if (this.#config.enableWallet) {
            await this.#wallet.initKeyPair(
                this.#config.keyPairPath,
                this.#readline_instance
            );
        }
        this.#state = new State(this.#store, this.#wallet, this.#config);
        this.#network = new Network(this.#state, this.#config, this.#wallet.address);

        await this.#state.ready();
        await this.#network.ready();
        await this.#stateEventsListener();

        if (this.#config.enableWallet) {
            printWalletInfo(this.#wallet.address, this.#state.writingKey, this.#state, this.#config.enableWallet);
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

        await printBalance(this.#wallet.address, this.#state, this.#config.enableWallet);
    }

    async _close() {
        console.log("Closing everything gracefully... This may take a moment.");

        this.#isClosing = true;
        await this.#network.close();

        await sleep(100);

        await this.#state.close();

        await sleep(100);

        if (this.#readline_instance) {
            const inputClosed = new Promise((resolve) =>
                this.#readline_instance.input.once("close", resolve)
            );
            const outputClosed = new Promise((resolve) =>
                this.#readline_instance.output.once("close", resolve)
            );

            this.#readline_instance.close();
            this.#readline_instance.input.destroy();
            this.#readline_instance.output.destroy();

            // Do not remove this. Without it, readline may close too quickly and still hang.
            await Promise.all([inputClosed, outputClosed]).catch((e) =>
                console.log("Error during closing readline stream:", e)
            );
        }

        await sleep(100);

        if (this.#store !== null) {
            await this.#store.close();
        }

        await sleep(100);
    }

    async broadcastPartialTransaction(partialTransactionPayload) {
        return await this.#network.validatorMessageOrchestrator.send(partialTransactionPayload);
    }

    async #setUpRoleAutomatically() {
        if (!this.#state.isWritable() && this.#config.enableRoleRequester) {
            console.log("Requesting writer role... This may take a moment.");
            await this.#requestWriterRole(false);
            setTimeout(async () => {
                await this.#requestWriterRole(true);
            }, 5_000);
            await sleep(5_000);
        }
    }

    #isAdmin(adminEntry) {
        if (!adminEntry || !this.#config.enableWallet) return false;
        return this.#wallet.address === adminEntry.address && b4a.equals(adminEntry.wk, this.#state.writingKey)
    }

    async #isAllowedToRequestRole(adminEntry, nodeEntry) {
        return nodeEntry?.isWhitelisted && !this.#isAdmin(adminEntry);
    }

    async #stateEventsListener() {
        this.#state.on(CustomEventType.IS_INDEXER, (publicKey) => {
            if (this.#network.validatorConnectionManager.exists(publicKey)) {
                this.#network.validatorConnectionManager.remove(publicKey)
            }
        })

        this.#state.on(CustomEventType.UNWRITABLE, (publicKey) => {
            if (this.#network.validatorConnectionManager.exists(publicKey)) {
                this.#network.validatorConnectionManager.remove(publicKey)
            }
        })
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

    async #handleAdminCreation() {
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

        const txValidity = await PeerWallet.blake3(this.#config.bootstrap);
        const addAdminMessage = await applyStateMessageFactory(this.#wallet, this.#config)
            .buildCompleteAddAdminMessage(
                this.#wallet.address,
                this.#state.writingKey,
                txValidity,
            )
        const encodedPayload = safeEncodeApplyOperation(addAdminMessage);

        await this.#state.append(encodedPayload);
    }

    async #handleAdminRecovery() {
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
            throw new Error("Failed to broadcast transaction after multiple attempts.");
        }

        console.info(`Transaction hash: ${adminRecoveryMessage.rao.tx}`);
    }

    async #handleWhitelistOperations() {
        if (!this.#config.enableWallet) {
            throw new Error("Cannot perform whitelisting - wallet is not enabled.");
        }

        const adminEntry = await this.#state.getAdminEntry();

        if (!this.#isAdmin(adminEntry)) {
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

    async #requestWriterRole(toAdd) {
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
                throw new Error("Failed to broadcast transaction after multiple attempts.");
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
            throw new Error("Failed to broadcast transaction after multiple attempts.");
        }

        console.info(`Transaction hash: ${assembledMessage.rao.tx}`);
    }

    async #updateWriterToIndexerRole(addressToUpdate, toAdd) {
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

        if (!this.#isAdmin(adminEntry) && !this.#state.isWritable()) {
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

    async #banValidator(addresstToBan) {
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

        if (!this.#isAdmin(adminEntry)) {
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

    async #deployBootstrap(externalBootstrap, channel) {
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
            throw new Error("Failed to broadcast transaction after multiple attempts.");
        }

        console.info(`Transaction hash: ${payload.bdo.tx}`);
        console.log(`Bootstrap ${externalBootstrap} deployment requested on channel ${channel}`);
        console.log('Bootstrap Deployment Fee Details:');
        console.log(`Fee: ${bigIntToDecimalString(feeBigInt)}`);
        console.log(`Expected Balance After Deployment: ${bigIntToDecimalString(senderBalance - feeBigInt)}`);

    }

    async #handleTransferOperation(recipientAddress, amount) {
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
        console.info('Transfer Details:');
        console.info(`Transaction hash ${payload.tro.tx}`)
        if (isSelfTransfer) {
            console.info('Self transfer - only fee will be deducted');
            console.info(`Fee: ${bigIntToDecimalString(feeBigInt)}`);
            console.info(`Total amount to send: ${bigIntToDecimalString(totalDeductedAmount)}`);
        } else {
            console.info(`Amount: ${bigIntToDecimalString(amountBigInt)}`);
            console.info(`Fee: ${bigIntToDecimalString(feeBigInt)}`);
            console.info(`Total: ${bigIntToDecimalString(totalDeductedAmount)}`);
        }
        console.log(`Expected Balance After Transfer: ${bigIntToDecimalString(expectedNewBalance)}`);
        const success = await this.broadcastPartialTransaction(payload);
        if (!success) {
            throw new Error("Failed to broadcast transfer transaction after multiple attempts.");
        } else {
            console.log(`Transfer transaction broadcasted successfully. Tx hash: ${payload.tro.tx}`);
        }

    }

    async #balanceMigrationOperation() {

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

        if (!this.#isAdmin(adminEntry)) {
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

    async #disableInitialization() {
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

        if (!this.#isAdmin(adminEntry)) {
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

    async interactiveMode() {
        if (this.#readline_instance === null) return;
        const rl = this.#readline_instance;

        printHelp(this.#config.isAdminMode);

        rl.on("line", async (input) => {
            try {
                await this.handleCommand(input.trim(), rl);
            } catch (err) {
                console.error(`${err}`);
            }
            rl.prompt();
        });

        rl.prompt();
    }

    async handleCommand(input, rl = null, payload = null) {
        const [command, ...parts] = input.split(" ");
        const exactHandlers = {
            "/help": async () => {
                printHelp(this.#config.isAdminMode);
            },
            "/exit": async () => {
                if (rl) rl.close();
                await this.close();
            },
            "/add_admin": async () => await this.#handleAdminCreation(),
            "/add_admin --recovery": async () => await this.#handleAdminRecovery(),
            "/add_whitelist": async () => await this.#handleWhitelistOperations(),
            "/add_writer": async () => await this.#requestWriterRole(true),
            "/remove_writer": async () => await this.#requestWriterRole(false),
            "/core": async () => await coreInfoCommand(this.#state),
            "/indexers_list": async () => console.log(await this.#state.getIndexersEntry()),
            "/validator_pool": () => this.#network.validatorConnectionManager.prettyPrint(),
            "/stats": async () => await verifyDag(
                this.#state,
                this.#network,
                this.#wallet,
                this.#state.writingKey
            ),
            "/balance_migration": async () => await this.#balanceMigrationOperation(),
            "/disable_initialization": async () => await this.#disableInitialization()
        };

        if (exactHandlers[command]) {
            const result = await exactHandlers[command]();
            if (rl) rl.prompt();
            return result;
        }

        if (input.startsWith("/node_status")) {
            const address = parts[0];
            const result = await nodeStatusCommand(this.#state, address);
            if (rl) rl.prompt();
            return result;
        }

        if (input.startsWith("/add_indexer")) {
            const address = parts[0];
            await this.#updateWriterToIndexerRole(address, true);
        } else if (input.startsWith("/remove_indexer")) {
            const address = parts[0];
            await this.#updateWriterToIndexerRole(address, false);
        } else if (input.startsWith("/ban_writer")) {
            const address = parts[0];
            await this.#banValidator(address);
        } else if (input.startsWith("/deployment")) {
            const bootstrapToDeploy = parts[0];
            const channel = parts[1] || randomBytes(32).toString("hex");
            if (channel.length !== 64 || !isHexString(channel)) {
                throw new Error("Channel must be a 32-byte hex string");
            }
            await this.#deployBootstrap(bootstrapToDeploy, channel);
        } else if (input.startsWith("/get_validator_addr")) {
            const wkHexString = parts[0];
            await getValidatorAddressCommand(this.#state, wkHexString, this.#config.addressPrefix);
        } else if (input.startsWith("/get_deployment")) {
            const bootstrapHex = parts[0];
            await getDeploymentCommand(this.#state, bootstrapHex, this.#config.addressLength);
        } else if (input.startsWith("/get_tx_info")) {
            const txHash = parts[0];
            await getTxInfoCommand(this.#state, txHash);
        } else if (input.startsWith("/transfer")) {
            const address = parts[0];
            const amount = parts[1];
            await this.#handleTransferOperation(address, amount);
        } else if (input.startsWith("/get_balance")) {
            const address = parts[0];
            const confirmedFlag = parts[1];
            const result = await getBalanceCommand(this.#state, address, confirmedFlag);
            if (rl) rl.prompt();
            return result;
        } else if (input.startsWith("/get_license_number")) {
            const address = parts[0];
            await getLicenseNumberCommand(this.#state, address);
        } else if (input.startsWith("/get_license_address")) {
            const licenseId = parseInt(parts[0]);
            await getLicenseAddressCommand(this.#state, licenseId);
        } else if (input.startsWith("/get_license_count")) {
            await getLicenseCountCommand(this.#state, this.#isAdmin.bind(this));
        } else if (input.startsWith("/get_txv")) {
            const result = await getTxvCommand(this.#state);
            if (rl) rl.prompt();
            return result;
        } else if (input.startsWith("/get_fee")) {
            const result = getFeeCommand(this.#state);
            if (rl) rl.prompt();
            return result;
        } else if (input.startsWith("/confirmed_length")) {
            const result = getConfirmedLengthCommand(this.#state);
            if (rl) rl.prompt();
            return result;
        } else if (input.startsWith("/unconfirmed_length")) {
            const result = getUnconfirmedLengthCommand(this.#state);
            if (rl) rl.prompt();
            return result;
        } else if (input.startsWith("/get_tx_payloads_bulk")) {
            if (!payload) {
                throw new Error("Missing payload for fetching tx payloads.");
            }
            const result = await getTxPayloadsBulkCommand(this.#state, payload, this.#config);
            if (rl) rl.prompt();
            return result;
        } else if (input.startsWith("/get_txs_hashes")) {
            const start = parseInt(parts[0]);
            const end = parseInt(parts[1]);
            const result = await getTxHashesCommand(this.#state, start, end);
            if (rl) rl.prompt();
            return result;
        } else if (input.startsWith("/get_tx_details")) {
            const hash = parts[0];
            const result = await getTxDetailsCommand(this.#state, hash, this.#config);
            if (rl) rl.prompt();
            return result;
        } else if (input.startsWith("/get_extended_tx_details")) {
            const hash = parts[0];
            const confirmed = parts[1] === "true";
            const result = await getExtendedTxDetailsCommand(this.#state, hash, confirmed, this.#config);
            if (rl) rl.prompt();
            return result;
        }

        if (rl) rl.prompt();
    }
}

export default MainSettlementBus;
