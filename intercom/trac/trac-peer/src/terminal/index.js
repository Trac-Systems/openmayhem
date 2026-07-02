/** @typedef {import('pear-interface')} */ /* global Pear */
import readline from 'readline';
import tty from 'tty';
import { TerminalHandlers } from './handlers.js';

class Terminal {
    #peer
    #handlers

    constructor(peer) {
        this.#peer = peer
        this.#handlers = new TerminalHandlers(peer)
    }

    printHelp() {
        console.log('Node started. Available commands:');
        console.log(' ');
        console.log('- Setup Commands:');
        console.log('- /add_admin | Works only once and only on the bootstrap node. Enter a peer public key (hex) to assign admin rights: \'/add_admin --address "<hex>"\'.');
        console.log('- /update_admin | Existing admins may transfer admin ownership. Enter "null" as address to waive admin rights for this peer entirely: \'/update_admin --address "<address>"\'.');
        console.log('- /add_indexer | Only admin. Enter a peer writer key to get included as indexer for this network: \'/add_indexer --key "<key>"\'.');
        console.log('- /add_writer | Only admin. Enter a peer writer key to get included as writer for this network: \'/add_writer --key "<key>"\'.');
        console.log('- /remove_writer | Only admin. Enter a peer writer key to get removed as writer or indexer for this network: \'/remove_writer --key "<key>"\'.');
        console.log('- /remove_indexer | Only admin. Alias of /remove_writer (removes indexer as well): \'/remove_indexer --key "<key>"\'.');
        console.log('- /set_auto_add_writers | Only admin. Allow any peer to join as writer automatically: \'/set_auto_add_writers --enabled 1\'');
        console.log('- /enable_transactions | Enable transactions.');
        console.log(' ');
        console.log('- Chat Commands:');
        console.log('- /set_chat_status | Only admin. Enable/disable the built-in chat system: \'/set_chat_status --enabled 1\'. The chat system is disabled by default.');
        console.log('- /post | Post a message: \'/post --message "Hello"\'. Chat must be enabled. Optionally use \'--reply_to <message id>\' to respond to a desired message.');
        console.log('- /set_nick | Change your nickname like this \'/set_nick --nick "Peter"\'. Chat must be enabled. Can be edited by admin and mods using the optional --user <address> flag.');
        console.log('- /mute_status | Only admin and mods. Mute or unmute a user by their address: \'/mute_status --user "<address>" --muted 1\'.');
        console.log('- /set_mod | Only admin. Set a user as mod: \'/set_mod --user "<address>" --mod 1\'.');
        console.log('- /delete_message | Delete a message: \'/delete_message --id 1\'. Chat must be enabled.');
        console.log('- /pin_message | Set the pin status of a message: \'/pin_message --id 1 --pin 1\'. Chat must be enabled.');
        console.log('- /unpin_message | Unpin a message by its pin id: \'/unpin_message --pin_id 1\'. Chat must be enabled.');
        console.log('- /enable_whitelist | Only admin. Enable/disable chat whitelists: \'/enable_whitelist --enabled 1\'.');
        console.log('- /set_whitelist_status | Only admin. Add/remove users to/from the chat whitelist: \'/set_whitelist_status --user "<address>" --status 1\'.');
        console.log(' ');
        console.log('- System Commands:');
        console.log('- /tx | Perform a contract transaction. The command flag contains contract commands (format is protocol dependent): \'/tx --command "<string>"\'. To simulate a tx, additionally use \'--sim 1\'.');
        console.log('- /deploy_subnet | Register this subnet in the MSB (required before TX settlement): \'/deploy_subnet\'.');
        console.log('- /stats | check system properties such as writer key, DAG, etc.');
        console.log('- /get_keys | prints your public and private keys. Be careful and never share your private key!');
        console.log('- /exit | Exit the program');
        console.log('- /help | This help text');
    
        this.#peer.protocol.instance.printOptions();
    }

    async start({ readlineInstance = null } = {}) {
        const peer = this.#peer;
        if (!peer) return;
        if (global.Pear !== undefined && global.Pear.config?.options?.type === 'desktop') return;
    
        let rl = readlineInstance;
        if (!rl) {
            try {
                rl = readline.createInterface({
                    input: new tty.ReadStream(0),
                    output: new tty.WriteStream(1),
                });
            } catch (_e) {
                try {
                    rl = readline.createInterface({ input: process.stdin, output: process.stdout });
                } catch (_e2) {
                    return;
                }
            }
        }
    
        this.printHelp();

        const commandHandlers = [
            { rule: (line) => line === '/stats', handler: (line) => this.#handlers.verifyDag(line) },
            { rule: (line) => line === '/help', handler: () => this.printHelp() },
            { rule: (line) => line === '/exit', handler: () => this.#handlers.exit({ rl }) },
            { rule: (line) => line === '/get_keys', handler: () => this.#handlers.getKeys() },
            { rule: (line) => line.startsWith('/tx'), handler: (line) => this.#handlers.tx(line) },
            { rule: (line) => line.startsWith('/add_indexer'), handler: (line) => this.#handlers.addIndexer(line) },
            { rule: (line) => line.startsWith('/add_writer'), handler: (line) => this.#handlers.addWriter(line) },
            { rule: (line) => line.startsWith('/remove_writer'), handler: (line) => this.#handlers.removeWriter(line) },
            { rule: (line) => line.startsWith('/remove_indexer'), handler: (line) => this.#handlers.removeIndexer(line) },
            { rule: (line) => line.startsWith('/add_admin'), handler: (line) => this.#handlers.addAdmin(line) },
            { rule: (line) => line.startsWith('/update_admin'), handler: (line) => this.#handlers.updateAdmin(line) },
            { rule: (line) => line.startsWith('/enable_transactions'), handler: (line) => this.#handlers.enableTransactions(line) },
            { rule: (line) => line.startsWith('/set_auto_add_writers'), handler: (line) => this.#handlers.setAutoAddWriters(line) },
            { rule: (line) => line.startsWith('/set_chat_status'), handler: (line) => this.#handlers.setChatStatus(line) },
            { rule: (line) => line.startsWith('/post'), handler: (line) => this.#handlers.postMessage(line) },
            { rule: (line) => line.startsWith('/set_nick'), handler: (line) => this.#handlers.setNick(line) },
            { rule: (line) => line.startsWith('/mute_status'), handler: (line) => this.#handlers.muteStatus(line) },
            { rule: (line) => line.startsWith('/pin_message'), handler: (line) => this.#handlers.pinMessage(line) },
            { rule: (line) => line.startsWith('/unpin_message'), handler: (line) => this.#handlers.unpinMessage(line) },
            { rule: (line) => line.startsWith('/set_mod'), handler: (line) => this.#handlers.setMod(line) },
            { rule: (line) => line.startsWith('/delete_message'), handler: (line) => this.#handlers.deleteMessage(line) },
            { rule: (line) => line.startsWith('/enable_whitelist'), handler: (line) => this.#handlers.enableWhitelist(line) },
            { rule: (line) => line.startsWith('/set_whitelist_status'), handler: (line) => this.#handlers.setWhitelistStatus(line) },
            { rule: (line) => line.startsWith('/deploy_subnet'), handler: (line) => this.#handlers.deploySubnet(line) },
            { rule: () => true, handler: (line) => peer.protocol.instance.customCommand(line) }
        ];

        rl.on('line', async (input) => {
            for (const { rule, handler } of commandHandlers) {
                if (!rule(input)) continue;
                try {
                    const result = await handler(input);
                    if (result?.exit) return;
                } catch (e) {
                    console.log('Command failed:', e.message);
                }
                break;
            }
            rl.prompt();
        });
    
        rl.prompt();
        return rl;
    }
}

export { Terminal };


