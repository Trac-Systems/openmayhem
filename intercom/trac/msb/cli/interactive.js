import ReadyResource from "ready-resource";
import process from "process";
import readline from "readline";
import { WalletProvider, exportWallet, importFromFile } from "trac-wallet";
import { MainSettlementBus } from "../src/index.js";
import { sleep } from "../src/utils/helpers.js";
import fileUtils from "../src/utils/fileUtils.js";
import { COMMANDS, CommandHandler } from "./commandHandler.js";

class Cli extends ReadyResource {
    #msb;
    #config;
    #commandHandlers;
    #readlineInstance;
    #wallet

    constructor(config) {
        super();
        this.#config = config;
    }

    async _open() {
        this.#readlineInstance = readline.createInterface({
            input: process.stdin,
            output: process.stdout,
            completer: line => this.#completeCommand(line) // This only works without Pear.
        });

        if (this.#config.enableWallet) {
            await fileUtils.ensureKeyPathDir(this.#config);
            await this.#initKeyPair()
        }

        this.#msb = new MainSettlementBus(this.#config, this.#wallet);
        await this.#msb.ready();
    }

    async _close() {
        if (this.#readlineInstance) {
            const inputClosed = new Promise((resolve) =>
                this.#readlineInstance.input.once("close", resolve)
            );
            const outputClosed = new Promise((resolve) =>
                this.#readlineInstance.output.once("close", resolve)
            );

            this.#readlineInstance.close();
            this.#readlineInstance.input.destroy();
            this.#readlineInstance.output.destroy();

            // Do not remove this. Without it, readline may close too quickly and still hang.
            await Promise.all([inputClosed, outputClosed]).catch((e) =>
                console.log("Error during closing readline stream:", e)
            );
        }

        await sleep(100);
    }

    startInteractiveMode() {
        console.log('RPC server will not be started.');

        this.#commandHandlers = new CommandHandler({
            config: this.#config,
            msb: this.#msb,
            handleClose: () => this.close(),
            wallet: this.#wallet
        });

        this.#msb.printHelp();

        this.#readlineInstance.on("line", async (input) => {
            try {
                await this.#handleCommand(input.trim());
            } catch (err) {
                console.error(`${err}`);
            }
            this.#readlineInstance.prompt();
        });

        this.#readlineInstance.prompt();
    }

    async #handleCommand(input) {
        return this.#commandHandlers.handle(input);
    }

    #completeCommand(line) {
        const commands = Object.values(COMMANDS);
        const hits = line
            ? commands.filter(command => command.startsWith(line))
            : commands;

        return [hits.length ? hits : commands, line];
    }

    async #initKeyPair() {
        try {
            if (fileUtils.verifyWalletPath(this.#config)) {
                this.#wallet = await importFromFile(this.#config.keyPairPath, undefined, this.#config.addressPrefix)
            } else {
                console.log("Key file was not found. How do you wish to proceed?")
                const wallet = await this.#setupKeypairInteractiveMode()
                if (!wallet) {
                    console.error("Invalid response type from keypair setup interactive menu")
                } else {
                    this.#wallet = wallet
                }
            }
        } catch (err) {
            console.error(err);
        }
    }

    async #setupKeypairInteractiveMode() {
        // this would be enable interactive mode (but there is an exception being swallowed so its better to act against the code)
        if (this.#readlineInstance !== null) {
            console.log([
                "[1]. Generate new keypair",
                "[2]. Restore keypair from 12 or 24-word mnemonic",
                "Your choice(1/ 2/):"
            ].join("\n"));

            let choice = await this.#awaitInput()

            try {
                switch (choice) {
                    case '1':
                        const wallet = await new WalletProvider(this.#config).generate({
                            derivationPath: this.#config.derivationPath
                        })
                        console.log([
                            "This is your mnemonic:",
                            wallet.mnemonic, 
                            "Please back it up in a safe location"

                        ].join("\n"))
                        exportWallet(wallet, this.#config.keyPairPath)
                        return wallet
                    case '2':
                        console.log("Enter your mnemonic phrase:");

                        let mnemonic = await this.#awaitInput()
                        try {
                            const wallet = await new WalletProvider(this.#config).fromMnemonic({
                                mnemonic,
                                derivationPath: this.#config.derivationPath
                            })
                            exportWallet(wallet, this.#config.keyPairPath)
                            return wallet
                        } catch {
                            console.log("Invalid mnemonic. Please check your 12 or 24 words and try again.");
                            return this.#setupKeypairInteractiveMode();
                        }
                    default:
                        console.log("Invalid choice. Please select again.");
                        return this.#setupKeypairInteractiveMode();
                }
            } catch {
                console.log("Invalid input. Please try again.");
                return this.#setupKeypairInteractiveMode();
            }
        }
    }

    async #awaitInput() {
        let choice = '';
        const anAction = async input => choice = input

        this.#readlineInstance.on('line', anAction)
        while ('' === choice) await sleep(1000);
        this.#readlineInstance.off('line', anAction);

        return choice
    }
}

export const startInteractiveMode = async (config) => {
    const cli = new Cli(config);
    await cli.ready();
    cli.startInteractiveMode();
};
