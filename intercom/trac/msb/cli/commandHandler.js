import { randomBytes } from "hypercore-crypto";
import { Handlers } from "./handlers.js";
import { isHexString } from "../src/utils/helpers.js";
import { bigIntToDecimalString } from "../src/utils/amountSerialization.js";

export const COMMANDS = {
    HELP: "/help",
    EXIT: "/exit",
    ADD_ADMIN: "/add_admin",
    ADD_ADMIN_RECOVERY: "/add_admin --recovery",
    ADD_WHITELIST: "/add_whitelist",
    ADD_WRITER: "/add_writer",
    REMOVE_WRITER: "/remove_writer",
    CORE: "/core",
    INDEXERS_LIST: "/indexers_list",
    VALIDATOR_POOL: "/validator_pool",
    STATS: "/stats",
    BALANCE_MIGRATION: "/balance_migration",
    DISABLE_INITIALIZATION: "/disable_initialization",
    NODE_STATUS: "/node_status",
    ADD_INDEXER: "/add_indexer",
    REMOVE_INDEXER: "/remove_indexer",
    BAN_WRITER: "/ban_writer",
    DEPLOYMENT: "/deployment",
    GET_VALIDATOR_ADDR: "/get_validator_addr",
    GET_DEPLOYMENT: "/get_deployment",
    GET_TX_INFO: "/get_tx_info",
    TRANSFER: "/transfer",
    GET_BALANCE: "/get_balance",
    GET_LICENSE_NUMBER: "/get_license_number",
    GET_LICENSE_ADDRESS: "/get_license_address",
    GET_LICENSE_COUNT: "/get_license_count",
    GET_TXV: "/get_txv",
    GET_FEE: "/get_fee",
    CONFIRMED_LENGTH: "/confirmed_length",
    UNCONFIRMED_LENGTH: "/unconfirmed_length",
    GET_TXS_HASHES: "/get_txs_hashes",
    GET_TX_DETAILS: "/get_tx_details",
    GET_EXTENDED_TX_DETAILS: "/get_extended_tx_details"
};

export class CommandHandler {
    #msb;
    #closeCli;
    #wallet;
    #handlers;
    #pendingConfirmation = null;

    constructor({ config, msb, handleClose, wallet }) {
        this.#msb = msb;
        this.#closeCli = handleClose;
        this.#wallet = wallet;
        this.#handlers = new Handlers(msb, config);
    }

    async handle(input) {
        if (this.#pendingConfirmation !== null) {
            return this.#handlePendingConfirmation(input);
        }

        const [command, ...parts] = input.split(" ");
        const context = { command, input, parts };
        const handlers = this.#getHandlers();
        const handler = handlers.find(({ evaluate }) => evaluate(context));

        if (handler) {
            return handler.process(context);
        }
    }

    #getHandlers() {
        return [
            {
                evaluate: ({ command }) => command === COMMANDS.HELP,
                process: async () => {
                    this.#msb.printHelp();
                }
            },
            {
                evaluate: ({ command }) => command === COMMANDS.EXIT,
                process: async () => {
                    await this.#closeCli();
                    await this.#msb.close();
                }
            },
            {
                evaluate: ({ input }) => input === COMMANDS.ADD_ADMIN_RECOVERY,
                process: async () => this.#msb.handleAdminRecovery()
            },
            {
                evaluate: ({ command }) => command === COMMANDS.ADD_ADMIN,
                process: async () => this.#msb.handleAdminCreation()
            },
            {
                evaluate: ({ command }) => command === COMMANDS.ADD_WHITELIST,
                process: async () => this.#msb.handleWhitelistOperations()
            },
            {
                evaluate: ({ command }) => command === COMMANDS.ADD_WRITER,
                process: async () => this.#msb.requestWriterRole(true)
            },
            {
                evaluate: ({ command }) => command === COMMANDS.REMOVE_WRITER,
                process: async () => this.#msb.requestWriterRole(false)
            },
            {
                evaluate: ({ command }) => command === COMMANDS.CORE,
                process: async () => this.#handlers.handleCoreInfo()
            },
            {
                evaluate: ({ command }) => command === COMMANDS.INDEXERS_LIST,
                process: async () => console.log(await this.#msb.state.getIndexersEntry())
            },
            {
                evaluate: ({ command }) => command === COMMANDS.VALIDATOR_POOL,
                process: async () => this.#msb.network.validatorConnectionManager.prettyPrint()
            },
            {
                evaluate: ({ command }) => command === COMMANDS.STATS,
                process: async () => this.#msb.verifyDag()
            },
            {
                evaluate: ({ command }) => command === COMMANDS.BALANCE_MIGRATION,
                process: async () => this.#msb.balanceMigrationOperation()
            },
            {
                evaluate: ({ command }) => command === COMMANDS.DISABLE_INITIALIZATION,
                process: async () => this.#msb.disableInitialization()
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.NODE_STATUS),
                process: async ({ parts }) => this.#handlers.handleNodeStatus(parts[0])
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.ADD_INDEXER),
                process: async ({ parts }) => this.#msb.updateWriterToIndexerRole(parts[0], true)
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.REMOVE_INDEXER),
                process: async ({ parts }) => this.#msb.updateWriterToIndexerRole(parts[0], false)
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.BAN_WRITER),
                process: async ({ parts }) => this.#msb.banValidator(parts[0])
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.DEPLOYMENT),
                process: async ({ parts }) => {
                    const channel = parts[1] || randomBytes(32).toString("hex");
                    if (!isHexString(channel, 64)) {
                        throw new Error("Channel must be a 32-byte hex string");
                    }
                    return this.#msb.deployBootstrap(parts[0], channel);
                }
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.GET_VALIDATOR_ADDR),
                process: async ({ parts }) => this.#handlers.handleValidatorAddress(parts[0])
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.GET_DEPLOYMENT),
                process: async ({ parts }) => this.#handlers.handleDeployment(parts[0])
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.GET_TX_INFO),
                process: async ({ parts }) => this.#handlers.handleTxInfo(parts[0])
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.TRANSFER),
                process: async ({ parts }) => this.#queueTransferConfirmation(parts[0], parts[1])
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.GET_BALANCE),
                process: async ({ parts }) => this.#handlers.handleBalance(parts[0] || this.#wallet?.address, parts[1])
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.GET_LICENSE_NUMBER),
                process: async ({ parts }) => this.#handlers.handleLicenseNumber(parts[0])
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.GET_LICENSE_ADDRESS),
                process: async ({ parts }) => this.#handlers.handleLicenseAddress(parseInt(parts[0]))
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.GET_LICENSE_COUNT),
                process: async () => this.#handlers.handleLicenseCount()
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.GET_TXV),
                process: async () => this.#handlers.handleTxv()
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.GET_FEE),
                process: async () => this.#handlers.handleFee()
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.CONFIRMED_LENGTH),
                process: async () => this.#handlers.handleConfirmedLength()
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.UNCONFIRMED_LENGTH),
                process: async () => this.#handlers.handleUnconfirmedLength()
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.GET_TXS_HASHES),
                process: async ({ parts }) => this.#handlers.handleTxHashes(
                    parseInt(parts[0]),
                    parseInt(parts[1])
                )
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.GET_TX_DETAILS),
                process: async ({ parts }) => this.#handlers.handleTxDetails(parts[0])
            },
            {
                evaluate: ({ input }) => input.startsWith(COMMANDS.GET_EXTENDED_TX_DETAILS),
                process: async ({ parts }) => this.#handlers.handleExtendedTxDetails(parts[0], parts[1] === "true")
            }
        ];
    }

    async #handlePendingConfirmation(input) {
        const normalizedInput = input.trim().toLowerCase();
        const pendingConfirmation = this.#pendingConfirmation;

        if (normalizedInput === "y" || normalizedInput === "yes") {
            this.#pendingConfirmation = null;

            try {
                return await pendingConfirmation.onConfirm();
            } catch (error) {
                const errorMessage = typeof error === "object" && error !== null && "message" in error
                    ? error.message
                    : `${error}`;
                console.error(`Transaction submission failed: ${errorMessage}`);
                console.log("Try again or use /help.");
            }

            return;
        }

        if (normalizedInput === "n" || normalizedInput === "no") {
            this.#pendingConfirmation = null;
            return pendingConfirmation.onDecline();
        }

        console.log('Invalid input. Please answer "y" or "n".');
        console.log(pendingConfirmation.prompt);
    }

    async #queueTransferConfirmation(recipientAddress, amount) {
        const preparedTransfer = await this.#msb.prepareTransferOperation(recipientAddress, amount);

        console.info("Transfer Details:");
        if (preparedTransfer.isSelfTransfer) {
            console.info("Self transfer - only fee will be deducted");
        }
        console.info(`Amount: ${bigIntToDecimalString(preparedTransfer.amountBigInt)}`);
        console.info(`Estimated transaction fee: ${bigIntToDecimalString(preparedTransfer.feeBigInt)}`);
        console.info(`Total transaction cost: ${bigIntToDecimalString(preparedTransfer.totalDeductedAmount)}`);
        console.info(`Current balance: ${bigIntToDecimalString(preparedTransfer.senderBalance)}`);
        console.info(`Balance after transaction: ${bigIntToDecimalString(preparedTransfer.expectedNewBalance)}`);

        this.#pendingConfirmation = {
            prompt: "Do you want to proceed? (y/n)",
            onConfirm: async () => this.#msb.submitPreparedTransferOperation(preparedTransfer),
            onDecline: async () => this.#msb.printHelp()
        };

        console.log(this.#pendingConfirmation.prompt);
    }
}
