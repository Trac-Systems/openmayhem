import {Protocol} from "trac-peer";
import { bufferToBigInt, bigIntToDecimalString } from "trac-msb/src/utils/amountSerialization.js";
import b4a from "b4a";
import PeerWallet from "trac-wallet";
import fs from "fs";

const stableStringify = (value) => {
    if (value === null || value === undefined) return 'null';
    if (typeof value !== 'object') return JSON.stringify(value);
    if (Array.isArray(value)) {
        return `[${value.map(stableStringify).join(',')}]`;
    }
    const keys = Object.keys(value).sort();
    return `{${keys.map((key) => `${JSON.stringify(key)}:${stableStringify(value[key])}`).join(',')}}`;
};

const normalizeInvitePayload = (payload) => {
    return {
        channel: String(payload?.channel ?? ''),
        inviteePubKey: String(payload?.inviteePubKey ?? '').trim().toLowerCase(),
        inviterPubKey: String(payload?.inviterPubKey ?? '').trim().toLowerCase(),
        inviterAddress: payload?.inviterAddress ?? null,
        issuedAt: Number(payload?.issuedAt),
        expiresAt: Number(payload?.expiresAt),
        nonce: String(payload?.nonce ?? ''),
        version: Number.isFinite(payload?.version) ? Number(payload.version) : 1,
    };
};

const normalizeWelcomePayload = (payload) => {
    return {
        channel: String(payload?.channel ?? ''),
        ownerPubKey: String(payload?.ownerPubKey ?? '').trim().toLowerCase(),
        text: String(payload?.text ?? ''),
        issuedAt: Number(payload?.issuedAt),
        version: Number.isFinite(payload?.version) ? Number(payload.version) : 1,
    };
};

const parseInviteArg = (raw) => {
    if (!raw) return null;
    let text = String(raw || '').trim();
    if (!text) return null;
    if (text.startsWith('@')) {
        try {
            text = fs.readFileSync(text.slice(1), 'utf8').trim();
        } catch (_e) {
            return null;
        }
    }
    if (text.startsWith('b64:')) text = text.slice(4);
    if (text.startsWith('{')) {
        try {
            return JSON.parse(text);
        } catch (_e) {}
    }
    try {
        const decoded = b4a.toString(b4a.from(text, 'base64'));
        return JSON.parse(decoded);
    } catch (_e) {}
    return null;
};

const parseWelcomeArg = (raw) => {
    if (!raw) return null;
    let text = String(raw || '').trim();
    if (!text) return null;
    if (text.startsWith('@')) {
        try {
            text = fs.readFileSync(text.slice(1), 'utf8').trim();
        } catch (_e) {
            return null;
        }
    }
    if (text.startsWith('b64:')) text = text.slice(4);
    if (text.startsWith('{')) {
        try {
            return JSON.parse(text);
        } catch (_e) {}
    }
    try {
        const decoded = b4a.toString(b4a.from(text, 'base64'));
        return JSON.parse(decoded);
    } catch (_e) {}
    return null;
};

class SampleProtocol extends Protocol{

    /**
     * Extending from Protocol inherits its capabilities and allows you to define your own protocol.
     * The protocol supports the corresponding contract. Both files come in pairs.
     *
     * Instances of this class do NOT run in contract context. The constructor is only called once on Protocol
     * instantiation.
     *
     * this.peer: an instance of the entire Peer class, the actual node that runs the contract and everything else.
     * this.base: the database engine, provides await this.base.view.get('key') to get unsigned data (not finalized data).
     * this.options: the option stack passed from Peer instance.
     *
     * @param peer
     * @param base
     * @param options
     */
    constructor(peer, base, options = {}) {
        // calling super and passing all parameters is required.
        super(peer, base, options);
    }

    /**
     * The Protocol superclass ProtocolApi instance already provides numerous api functions.
     * You can extend the built-in api based on your protocol requirements.
     *
     * @returns {Promise<void>}
     */
    async extendApi(){
        this.api.getSampleData = function(){
            return 'Some sample data';
        }
    }

    /**
     * In order for a transaction to successfully trigger,
     * you need to create a mapping for the incoming tx command,
     * pointing at the contract function to execute.
     *
     * You can perform basic sanitization here, but do not use it to protect contract execution.
     * Instead, use the built-in schema support for in-contract sanitization instead
     * (Contract.addSchema() in contract constructor).
     *
     * @param command
     * @returns {{type: string, value: *}|null}
     */
    mapTxCommand(command){
        // prepare the payload
        let obj = { type : '', value : null };
        /*
        Triggering contract function in terminal will look like this:

        /tx --command 'something'

        You can also simulate a tx prior broadcast

        /tx --command 'something' --sim 1

        To programmatically execute a transaction from "outside",
        the api function "this.api.tx()" needs to be exposed by adding
        "api_tx_exposed : true" to the Peer instance options.
        Once exposed, it can be used directly through peer.protocol_instance.api.tx()

        Please study the superclass of this Protocol and Protocol.api to learn more.
        */
        if(command === 'something'){
            // type points at the "storeSomething" function in the contract.
            obj.type = 'storeSomething';
            // value can be null as there is no other payload, but the property must exist.
            obj.value = null;
            // return the payload to be used in your contract
            return obj;
        } else if (command === 'read_snapshot') {
            obj.type = 'readSnapshot';
            obj.value = null;
            return obj;
        } else if (command === 'read_chat_last') {
            obj.type = 'readChatLast';
            obj.value = null;
            return obj;
        } else if (command === 'read_timer') {
            obj.type = 'readTimer';
            obj.value = null;
            return obj;
        } else {
            /*
            now we assume our protocol allows to submit a json string with information
            what to do (the op) then we pass the parsed object to the value.
            the accepted json string can be executed as tx like this:

            /tx --command '{ "op" : "do_something", "some_key" : "some_data" }'

            Of course we can simulate this, as well:

            /tx --command '{ "op" : "do_something", "some_key" : "some_data" }' --sim 1
            */
            const json = this.safeJsonParse(command);
            if(json.op !== undefined && json.op === 'do_something'){
                obj.type = 'submitSomething';
                obj.value = json;
                return obj;
            } else if (json.op !== undefined && json.op === 'read_key') {
                obj.type = 'readKey';
                obj.value = json;
                return obj;
            } else if (json.op !== undefined && json.op === 'read_chat_last') {
                obj.type = 'readChatLast';
                obj.value = null;
                return obj;
            } else if (json.op !== undefined && json.op === 'read_timer') {
                obj.type = 'readTimer';
                obj.value = null;
                return obj;
            }
        }
        // return null if no case matches.
        // if you do not return null, your protocol might behave unexpected.
        return null;
    }

    /**
     * Prints additional options for your protocol underneath the system ones in terminal.
     *
     * @returns {Promise<void>}
     */
    async printOptions(){
        console.log(' ');
        console.log('- Sample Commands:');
        console.log("- /print | use this flag to print some text to the terminal: '--text \"I am printing\"");
        console.log('- /get --key "<key>" [--confirmed true|false] | reads subnet state key (confirmed defaults to true).');
        console.log('- /msb | prints MSB txv + lengths (local MSB node view).');
        console.log('- /tx --command "read_chat_last" | prints last chat message captured by contract.');
        console.log('- /tx --command "read_timer" | prints current timer feature value.');
        console.log('- /sc_join --channel "<name>" | join an ephemeral sidechannel (no autobase).');
        console.log('- /sc_open --channel "<name>" [--via "<channel>"] [--invite <json|b64|@file>] [--welcome <json|b64|@file>] | request others to open a sidechannel.');
        console.log('- /sc_send --channel "<name>" --message "<text>" [--invite <json|b64|@file>] | send message over sidechannel.');
        console.log('- /sc_invite --channel "<name>" --pubkey "<peer-pubkey-hex>" [--ttl <sec>] [--welcome <json|b64|@file>] | create a signed invite.');
        console.log('- /sc_welcome --channel "<name>" --text "<message>" | create a signed welcome.');
        console.log('- /sc_stats | show sidechannel channels + connection count.');
        // further protocol specific options go here
    }

    /**
     * Extend the terminal system commands and execute your custom ones for your protocol.
     * This is not transaction execution itself (though can be used for it based on your requirements).
     * For transactions, use the built-in /tx command in combination with command mapping (see above)
     *
     * @param input
     * @returns {Promise<void>}
     */
    async customCommand(input) {
        await super.tokenizeInput(input);
        if (this.input.startsWith("/get")) {
            const m = input.match(/(?:^|\s)--key(?:=|\s+)(\"[^\"]+\"|'[^']+'|\S+)/);
            const raw = m ? m[1].trim() : null;
            if (!raw) {
                console.log('Usage: /get --key "<hyperbee-key>" [--confirmed true|false] [--unconfirmed 1]');
                return;
            }
            const key = raw.replace(/^\"(.*)\"$/, "$1").replace(/^'(.*)'$/, "$1");
            const confirmedMatch = input.match(/(?:^|\s)--confirmed(?:=|\s+)(\S+)/);
            const unconfirmedMatch = input.match(/(?:^|\s)--unconfirmed(?:=|\s+)?(\S+)?/);
            const confirmed = unconfirmedMatch ? false : confirmedMatch ? confirmedMatch[1] === "true" || confirmedMatch[1] === "1" : true;
            const v = confirmed ? await this.getSigned(key) : await this.get(key);
            console.log(v);
            return;
        }
        if (this.input.startsWith("/msb")) {
            const txv = await this.peer.msbClient.getTxvHex();
            const peerMsbAddress = this.peer.msbClient.pubKeyHexToAddress(this.peer.wallet.publicKey);
            const entry = await this.peer.msbClient.getNodeEntryUnsigned(peerMsbAddress);
            const balance = entry?.balance ? bigIntToDecimalString(bufferToBigInt(entry.balance)) : 0;
            const feeBuf = this.peer.msbClient.getFee();
            const fee = feeBuf ? bigIntToDecimalString(bufferToBigInt(feeBuf)) : 0;
            const validators = this.peer.msbClient.getConnectedValidatorsCount();
            console.log({
                networkId: this.peer.msbClient.networkId,
                msbBootstrap: this.peer.msbClient.bootstrapHex,
                txv,
                msbSignedLength: this.peer.msbClient.getSignedLength(),
                msbUnsignedLength: this.peer.msbClient.getUnsignedLength(),
                connectedValidators: validators,
                peerMsbAddress,
                peerMsbBalance: balance,
                msbFee: fee,
            });
            return;
        }
        if (this.input.startsWith("/sc_join")) {
            const args = this.parseArgs(input);
            const name = args.channel || args.ch || args.name;
            const inviteArg = args.invite || args.invite_b64 || args.invitebase64;
            const welcomeArg = args.welcome || args.welcome_b64 || args.welcomebase64;
            if (!name) {
                console.log('Usage: /sc_join --channel "<name>" [--invite <json|b64|@file>] [--welcome <json|b64|@file>]');
                return;
            }
            if (!this.peer.sidechannel) {
                console.log('Sidechannel not initialized.');
                return;
            }
            let invite = null;
            if (inviteArg) {
                invite = parseInviteArg(inviteArg);
                if (!invite) {
                    console.log('Invalid invite. Pass JSON, base64, or @file.');
                    return;
                }
            }
            let welcome = null;
            if (welcomeArg) {
                welcome = parseWelcomeArg(welcomeArg);
                if (!welcome) {
                    console.log('Invalid welcome. Pass JSON, base64, or @file.');
                    return;
                }
            }
            if (invite || welcome) {
                this.peer.sidechannel.acceptInvite(String(name), invite, welcome);
            }
            const ok = await this.peer.sidechannel.addChannel(String(name));
            if (!ok) {
                console.log('Join denied (invite required or invalid).');
                return;
            }
            console.log('Joined sidechannel:', name);
            return;
        }
        if (this.input.startsWith("/sc_send")) {
            const args = this.parseArgs(input);
            const name = args.channel || args.ch || args.name;
            const message = args.message || args.msg;
            const inviteArg = args.invite || args.invite_b64 || args.invitebase64;
            const welcomeArg = args.welcome || args.welcome_b64 || args.welcomebase64;
            if (!name || message === undefined) {
                console.log('Usage: /sc_send --channel "<name>" --message "<text>" [--invite <json|b64|@file>] [--welcome <json|b64|@file>]');
                return;
            }
            if (!this.peer.sidechannel) {
                console.log('Sidechannel not initialized.');
                return;
            }
            let invite = null;
            if (inviteArg) {
                invite = parseInviteArg(inviteArg);
                if (!invite) {
                    console.log('Invalid invite. Pass JSON, base64, or @file.');
                    return;
                }
            }
            let welcome = null;
            if (welcomeArg) {
                welcome = parseWelcomeArg(welcomeArg);
                if (!welcome) {
                    console.log('Invalid welcome. Pass JSON, base64, or @file.');
                    return;
                }
            }
            if (invite || welcome) {
                this.peer.sidechannel.acceptInvite(String(name), invite, welcome);
            }
            const ok = await this.peer.sidechannel.addChannel(String(name));
            if (!ok) {
                console.log('Send denied (invite required or invalid).');
                return;
            }
            const sent = this.peer.sidechannel.broadcast(String(name), message, invite ? { invite } : undefined);
            if (!sent) {
                console.log('Send denied (owner-only or invite required).');
            }
            return;
        }
        if (this.input.startsWith("/sc_open")) {
            const args = this.parseArgs(input);
            const name = args.channel || args.ch || args.name;
            const via = args.via || args.channel_via;
            const inviteArg = args.invite || args.invite_b64 || args.invitebase64;
            const welcomeArg = args.welcome || args.welcome_b64 || args.welcomebase64;
            if (!name) {
                console.log('Usage: /sc_open --channel "<name>" [--via "<channel>"] [--invite <json|b64|@file>] [--welcome <json|b64|@file>]');
                return;
            }
            if (!this.peer.sidechannel) {
                console.log('Sidechannel not initialized.');
                return;
            }
            let invite = null;
            if (inviteArg) {
                invite = parseInviteArg(inviteArg);
                if (!invite) {
                    console.log('Invalid invite. Pass JSON, base64, or @file.');
                    return;
                }
            }
            let welcome = null;
            if (welcomeArg) {
                welcome = parseWelcomeArg(welcomeArg);
                if (!welcome) {
                    console.log('Invalid welcome. Pass JSON, base64, or @file.');
                    return;
                }
            } else if (typeof this.peer.sidechannel.getWelcome === 'function') {
                welcome = this.peer.sidechannel.getWelcome(String(name));
            }
            const viaChannel = via || this.peer.sidechannel.entryChannel || null;
            if (!viaChannel) {
                console.log('No entry channel configured. Pass --via "<channel>".');
                return;
            }
            this.peer.sidechannel.requestOpen(String(name), String(viaChannel), invite, welcome);
            console.log('Requested channel:', name);
            return;
        }
        if (this.input.startsWith("/sc_invite")) {
            const args = this.parseArgs(input);
            const channel = args.channel || args.ch || args.name;
            const invitee = args.pubkey || args.invitee || args.peer || args.key;
            const ttlRaw = args.ttl || args.ttl_sec || args.ttl_s;
            const welcomeArg = args.welcome || args.welcome_b64 || args.welcomebase64;
            if (!channel || !invitee) {
                console.log('Usage: /sc_invite --channel "<name>" --pubkey "<peer-pubkey-hex>" [--ttl <sec>] [--welcome <json|b64|@file>]');
                return;
            }
            if (!this.peer.sidechannel) {
                console.log('Sidechannel not initialized.');
                return;
            }
            if (this.peer?.wallet?.ready) {
                try {
                    await this.peer.wallet.ready;
                } catch (_e) {}
            }
            const walletPub = this.peer?.wallet?.publicKey;
            const inviterPubKey = walletPub
                ? typeof walletPub === 'string'
                    ? walletPub.trim().toLowerCase()
                    : b4a.toString(walletPub, 'hex')
                : null;
            if (!inviterPubKey) {
                console.log('Wallet not ready; cannot sign invite.');
                return;
            }
            let inviterAddress = null;
            try {
                if (this.peer?.msbClient) {
                    inviterAddress = this.peer.msbClient.pubKeyHexToAddress(inviterPubKey);
                }
            } catch (_e) {}
            const issuedAt = Date.now();
            let ttlMs = null;
            if (ttlRaw !== undefined) {
                const ttlSec = Number.parseInt(String(ttlRaw), 10);
                ttlMs = Number.isFinite(ttlSec) ? Math.max(ttlSec, 0) * 1000 : null;
            } else if (Number.isFinite(this.peer.sidechannel.inviteTtlMs) && this.peer.sidechannel.inviteTtlMs > 0) {
                ttlMs = this.peer.sidechannel.inviteTtlMs;
            } else {
                ttlMs = 0;
            }
            if (!ttlMs || ttlMs <= 0) {
                console.log('Invite TTL is required. Pass --ttl <sec> or set --sidechannel-invite-ttl.');
                return;
            }
            const expiresAt = issuedAt + ttlMs;
            const payload = normalizeInvitePayload({
                channel: String(channel),
                inviteePubKey: String(invitee).trim().toLowerCase(),
                inviterPubKey,
                inviterAddress,
                issuedAt,
                expiresAt,
                nonce: Math.random().toString(36).slice(2, 10),
                version: 1,
            });
            const message = stableStringify(payload);
            const msgBuf = b4a.from(message);
            let sig = this.peer.wallet.sign(msgBuf);
            let sigHex = '';
            if (typeof sig === 'string') {
                sigHex = sig;
            } else if (sig && sig.length > 0) {
                sigHex = b4a.toString(sig, 'hex');
            }
            if (!sigHex) {
                const walletSecret = this.peer?.wallet?.secretKey;
                const secretBuf = walletSecret
                    ? b4a.isBuffer(walletSecret)
                        ? walletSecret
                        : typeof walletSecret === 'string'
                            ? b4a.from(walletSecret, 'hex')
                            : b4a.from(walletSecret)
                    : null;
                if (secretBuf) {
                    const sigBuf = PeerWallet.sign(msgBuf, secretBuf);
                    if (sigBuf && sigBuf.length > 0) {
                        sigHex = b4a.toString(sigBuf, 'hex');
                    }
                }
            }
            let welcome = null;
            if (welcomeArg) {
                welcome = parseWelcomeArg(welcomeArg);
                if (!welcome) {
                    console.log('Invalid welcome. Pass JSON, base64, or @file.');
                    return;
                }
            } else if (typeof this.peer.sidechannel.getWelcome === 'function') {
                welcome = this.peer.sidechannel.getWelcome(String(channel));
            }
            const invite = { payload, sig: sigHex, welcome: welcome || undefined };
            const inviteJson = JSON.stringify(invite);
            const inviteB64 = b4a.toString(b4a.from(inviteJson), 'base64');
            if (!sigHex) {
                console.log('Failed to sign invite; wallet secret key unavailable.');
                return;
            }
            console.log(inviteJson);
            console.log('invite_b64:', inviteB64);
            return;
        }
        if (this.input.startsWith("/sc_welcome")) {
            const args = this.parseArgs(input);
            const channel = args.channel || args.ch || args.name;
            const text = args.text || args.message || args.msg;
            if (!channel || text === undefined) {
                console.log('Usage: /sc_welcome --channel "<name>" --text "<message>"');
                return;
            }
            if (!this.peer.sidechannel) {
                console.log('Sidechannel not initialized.');
                return;
            }
            if (this.peer?.wallet?.ready) {
                try {
                    await this.peer.wallet.ready;
                } catch (_e) {}
            }
            const walletPub = this.peer?.wallet?.publicKey;
            const ownerPubKey = walletPub
                ? typeof walletPub === 'string'
                    ? walletPub.trim().toLowerCase()
                    : b4a.toString(walletPub, 'hex')
                : null;
            if (!ownerPubKey) {
                console.log('Wallet not ready; cannot sign welcome.');
                return;
            }
            const payload = normalizeWelcomePayload({
                channel: String(channel),
                ownerPubKey,
                text: String(text),
                issuedAt: Date.now(),
                version: 1,
            });
            const message = stableStringify(payload);
            const msgBuf = b4a.from(message);
            let sig = this.peer.wallet.sign(msgBuf);
            let sigHex = '';
            if (typeof sig === 'string') {
                sigHex = sig;
            } else if (sig && sig.length > 0) {
                sigHex = b4a.toString(sig, 'hex');
            }
            if (!sigHex) {
                const walletSecret = this.peer?.wallet?.secretKey;
                const secretBuf = walletSecret
                    ? b4a.isBuffer(walletSecret)
                        ? walletSecret
                        : typeof walletSecret === 'string'
                            ? b4a.from(walletSecret, 'hex')
                            : b4a.from(walletSecret)
                    : null;
                if (secretBuf) {
                    const sigBuf = PeerWallet.sign(msgBuf, secretBuf);
                    if (sigBuf && sigBuf.length > 0) {
                        sigHex = b4a.toString(sigBuf, 'hex');
                    }
                }
            }
            if (!sigHex) {
                console.log('Failed to sign welcome; wallet secret key unavailable.');
                return;
            }
            const welcome = { payload, sig: sigHex };
            // Store the welcome in-memory so the owner peer can auto-send it to new connections
            // without requiring a restart (and so /sc_invite can embed it by default).
            try {
                this.peer.sidechannel.acceptInvite(String(channel), null, welcome);
            } catch (_e) {}
            const welcomeJson = JSON.stringify(welcome);
            const welcomeB64 = b4a.toString(b4a.from(welcomeJson), 'base64');
            console.log(welcomeJson);
            console.log('welcome_b64:', welcomeB64);
            return;
        }
        if (this.input.startsWith("/sc_stats")) {
            if (!this.peer.sidechannel) {
                console.log('Sidechannel not initialized.');
                return;
            }
            const channels = Array.from(this.peer.sidechannel.channels.keys());
            const connectionCount = this.peer.sidechannel.connections.size;
            console.log({ channels, connectionCount });
            return;
        }
        if (this.input.startsWith("/print")) {
            const splitted = this.parseArgs(input);
            console.log(splitted.text);
        }
    }
}

export default SampleProtocol;
