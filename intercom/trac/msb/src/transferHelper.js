import fs from 'fs';
import path from 'path';

import { MainSettlementBus } from './index.js';
import { createConfig, ENV } from './config/env.js';
import { bufferToBigInt, bigIntToDecimalString } from './utils/amountSerialization.js';

function usage() {
    return `Usage:
  transfer --network <mainnet|testnet1> --stores-directory <path> --store-name <name> --to <address> --amount <tnk> [--timeout-seconds <n>] [--expected-balance-before <tnk>]
  batch-transfer --network <mainnet|testnet1> --stores-directory <path> --store-name <name> (--outputs-json <json> | --outputs-file <path>) [--timeout-seconds <n>] [--expected-balance-before <tnk>]`;
}

function fail(message) {
    throw new Error(message);
}

function takeOption(args, name) {
    const idx = args.indexOf(name);
    if (idx === -1) return null;
    const value = args[idx + 1];
    if (value === undefined || value.startsWith('--')) fail(`Missing value for ${name}.`);
    args.splice(idx, 2);
    return value;
}

function parseBatchOutputs(outputsJsonArg, outputsFile) {
    const raw = outputsJsonArg ?? fs.readFileSync(path.resolve(outputsFile), 'utf8');
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed) || parsed.length === 0) {
        throw new Error('batch outputs must be a non-empty JSON array');
    }
    return parsed;
}

function defaultStderr() {
    if (typeof process !== 'undefined' && process.stderr?.write) return process.stderr;
    return {
        write(message) {
            console.error(String(message).replace(/\n$/, ''));
        }
    };
}

export async function runTransferHelper(rawArgs, options = {}) {
    const stderr = options.stderr ?? defaultStderr();
    const args = [...rawArgs];
    const command = args.shift();
    if (command !== 'transfer' && command !== 'batch-transfer') {
        fail(`${usage()}`);
    }

    const network = takeOption(args, '--network') ?? fail('Missing --network.');
    const storesDirectory = takeOption(args, '--stores-directory') ?? fail('Missing --stores-directory.');
    const storeName = takeOption(args, '--store-name') ?? fail('Missing --store-name.');
    const to = command === 'transfer' ? takeOption(args, '--to') ?? fail('Missing --to.') : null;
    const amount = command === 'transfer' ? takeOption(args, '--amount') ?? fail('Missing --amount.') : null;
    const outputsJsonArg = command === 'batch-transfer' ? takeOption(args, '--outputs-json') : null;
    const outputsFile = command === 'batch-transfer' ? takeOption(args, '--outputs-file') : null;
    const timeoutSeconds = Number.parseInt(takeOption(args, '--timeout-seconds') ?? '180', 10);
    const expectedBalanceBefore = takeOption(args, '--expected-balance-before');

    if (args.length > 0) fail(`Unknown argument: ${args[0]}`);
    if (command === 'batch-transfer' && Boolean(outputsJsonArg) === Boolean(outputsFile)) {
        fail('Pass exactly one of --outputs-json or --outputs-file.');
    }
    if (!Number.isSafeInteger(timeoutSeconds) || timeoutSeconds <= 0) {
        fail('--timeout-seconds must be a positive safe integer.');
    }

    const env = {
        mainnet: ENV.MAINNET,
        testnet1: ENV.TESTNET1,
        testnet: ENV.TESTNET1,
    }[String(network).trim().toLowerCase()];
    if (!env) fail('--network must be mainnet or testnet1.');

    const originalLog = console.log;
    const originalInfo = console.info;
    const captured = [];
    const redirect = (...items) => {
        const line = items.map((item) => (
            typeof item === 'string' ? item : JSON.stringify(item)
        )).join(' ');
        captured.push(line);
        stderr.write(`${line}\n`);
    };
    console.log = redirect;
    console.info = redirect;

    const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
    const balanceDecimal = (entry) => (
        entry?.balance ? bigIntToDecimalString(bufferToBigInt(entry.balance)) : '0'
    );

    const normalizedStoresDirectory = path.resolve(storesDirectory) + path.sep;
    const config = createConfig(env, {
        storeName,
        storesDirectory: normalizedStoresDirectory,
        enableInteractiveMode: false,
        enableWallet: true,
    });

    const msb = new MainSettlementBus(config);
    try {
        await msb.ready();
        const from = msb.wallet.address;
        let entry = null;
        let validators = 0;
        for (let waited = 0; waited <= timeoutSeconds; waited += 1) {
            entry = await msb.state.getNodeEntry(from);
            validators = msb.network.validatorConnectionManager.connectionCount();
            if (entry && validators > 0) break;
            if (waited % 5 === 0) {
                redirect(`MSB transfer preflight ${waited}s: sender_balance=${entry ? balanceDecimal(entry) : 'syncing'} validator_connections=${validators}`);
            }
            await sleep(1000);
        }
        if (!entry) throw new Error('sender account did not sync before timeout');
        const beforeBalance = balanceDecimal(entry);
        if (expectedBalanceBefore && beforeBalance !== expectedBalanceBefore) {
            throw new Error(`sender balance is ${beforeBalance}, expected ${expectedBalanceBefore}; refusing transfer retry`);
        }
        if (validators <= 0) throw new Error('no validator connection before timeout');

        let outputs = null;
        if (command === 'batch-transfer') {
            outputs = parseBatchOutputs(outputsJsonArg, outputsFile);
            await msb.handleCommand(`/transfer_batch ${JSON.stringify(outputs)}`);
        } else {
            await msb.handleCommand(`/transfer ${to} ${amount}`);
        }
        const joined = captured.join('\n');
        const txHash = joined.match(/Batch transfer transaction broadcasted successfully\. Tx hash: ([0-9a-f]{64})/i)?.[1]
            ?? joined.match(/Transfer transaction broadcasted successfully\. Tx hash: ([0-9a-f]{64})/i)?.[1]
            ?? joined.match(/Transaction hash:?\s+([0-9a-f]{64})/i)?.[1]
            ?? null;
        if (!txHash) throw new Error('transfer broadcast did not expose a tx hash');
        return {
            ok: true,
            command,
            network: String(network).trim().toLowerCase() === 'testnet' ? 'testnet1' : String(network).trim().toLowerCase(),
            from,
            ...(command === 'batch-transfer' ? { outputs } : { to, amount }),
            tx_hash: txHash,
            before_balance: beforeBalance,
            validator_connections: validators,
        };
    } finally {
        console.log = originalLog;
        console.info = originalInfo;
        try {
            await msb.close();
        } catch (_error) {}
    }
}
