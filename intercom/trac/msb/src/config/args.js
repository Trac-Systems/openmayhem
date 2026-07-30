import { createConfig, ENV } from './env.js';

export const getArguments = (runtime = globalThis) => {
    const pearRuntime = runtime.Pear;
    const pearApp = pearRuntime?.app ?? pearRuntime?.config;
    const bareArgs = runtime.Bare?.argv?.slice(2);
    const processArgs = runtime.process?.argv?.slice(2) ?? [];
    return pearApp?.args ?? bareArgs ?? processArgs;
};

const getOptionValue = (args, flag) => {
    const index = args.indexOf(flag);
    if (index === -1) {
        return undefined;
    }

    const value = args[index + 1];
    if (value === undefined || String(value).startsWith('--')) {
        throw new Error(`MainSettlementBus CLI: ${flag} requires a value.`);
    }

    return value;
};

const parseNonEmptyStringOption = (args, flag) => {
    const value = getOptionValue(args, flag);
    if (value === undefined) {
        return undefined;
    }

    if (typeof value !== 'string' || value.trim().length === 0) {
        throw new Error(`MainSettlementBus CLI: ${flag} must be a non-empty string.`);
    }

    return value;
};

const parsePortOption = (args) => {
    const value = getOptionValue(args, '--port');
    if (value === undefined) {
        return undefined;
    }

    if (!/^\d+$/.test(value)) {
        throw new Error('MainSettlementBus CLI: --port must be an integer in range 1-65535.');
    }

    const port = Number(value);
    if (!Number.isInteger(port) || port < 1 || port > 65535) {
        throw new Error('MainSettlementBus CLI: --port must be an integer in range 1-65535.');
    }

    return port;
};

export const resolveEnvironment = (args = []) => {
    const network = getOptionValue(args, '--network');
    if (network === undefined) {
        return ENV.MAINNET;
    }

    if (network === ENV.MAINNET) return ENV.MAINNET;
    if (network === ENV.DEVELOPMENT) return ENV.DEVELOPMENT;
    if (network === ENV.TESTNET1 || network === 'testnet') return ENV.TESTNET1;

    throw new Error('MainSettlementBus CLI: --network must be one of: mainnet, development, testnet.');
};

export const isRpcEnabled = () => {
    const args = getArguments();
    return args.includes('--rpc');
};

export const resolveConfig = () => {
    const args = getArguments();
    const runRpc = isRpcEnabled();
    const selectedEnv = resolveEnvironment(args);
    const storesDirectory = parseNonEmptyStringOption(args, '--stores-directory');
    const host = runRpc ? parseNonEmptyStringOption(args, '--host') : undefined;
    const port = runRpc ? parsePortOption(args) : undefined;

    const rpc = {
        storesDirectory,
        enableWallet: false,
        enableInteractiveMode: false,
        host,
        port
    };

    const options = runRpc ? rpc : { storesDirectory };

    return createConfig(selectedEnv, options);
};
