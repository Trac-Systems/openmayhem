import test from 'brittle';

import {
    getArguments,
    isRpcEnabled,
    resolveConfig,
    resolveEnvironment
} from '../../../src/config/args.js';
import { ENV } from '../../../src/config/env.js';

async function withArgs(args, fn) {
    const hadPear = Object.prototype.hasOwnProperty.call(globalThis, 'Pear');
    const previousPear = globalThis.Pear;
    const hasProcess = typeof process !== 'undefined';
    const previousArgv = hasProcess ? process.argv : null;

    globalThis.Pear = {
        app: { args },
        config: { args }
    };

    if (hasProcess) {
        process.argv = ['node', 'msb.mjs', ...args];
    }

    try {
        await fn();
    } finally {
        if (hasProcess) {
            process.argv = previousArgv;
        }
        if (hadPear) {
            globalThis.Pear = previousPear;
        } else {
            delete globalThis.Pear;
        }
    }
}

test('args: resolveEnvironment defaults to mainnet and accepts testnet alias', t => {
    t.is(resolveEnvironment([]), ENV.MAINNET);
    t.is(resolveEnvironment(['--network', 'testnet']), ENV.TESTNET1);
    t.is(resolveEnvironment(['--network', 'testnet1']), ENV.TESTNET1);
});

test('args: resolveEnvironment rejects invalid and missing network values', t => {
    t.exception(
        () => resolveEnvironment(['--network']),
        /MainSettlementBus CLI: --network requires a value\./
    );

    t.exception(
        () => resolveEnvironment(['--network', 'staging']),
        /MainSettlementBus CLI: --network must be one of: mainnet, development, testnet\./
    );
});

test('args: resolveConfig validates startup CLI values for RPC mode', async t => {
    await withArgs([
        '--rpc',
        '--network', 'testnet1',
        '--stores-directory', 'custom-stores',
        '--host', '0.0.0.0',
        '--port', '6000'
    ], async () => {
        const config = resolveConfig();

        t.ok(isRpcEnabled());
        t.is(config.host, '0.0.0.0');
        t.is(config.port, 6000);
        t.is(config.storesDirectory, 'custom-stores');
        t.is(config.enableWallet, false);
        t.is(config.enableInteractiveMode, false);
        t.is(config.networkId, 919);
    });
});

test('args: resolveConfig validates startup CLI values for interactive mode', async t => {
    await withArgs(['--stores-directory', 'interactive-stores'], async () => {
        const config = resolveConfig();

        t.absent(isRpcEnabled());
        t.is(config.storesDirectory, 'interactive-stores');
        t.is(config.enableWallet, true);
        t.is(config.enableInteractiveMode, true);
    });
});

test('args: resolveConfig rejects missing values after user-provided flags', async t => {
    await withArgs(['--stores-directory'], async () => {
        t.exception(
            () => resolveConfig(),
            /MainSettlementBus CLI: --stores-directory requires a value\./
        );
    });

    await withArgs(['--rpc', '--host'], async () => {
        t.exception(
            () => resolveConfig(),
            /MainSettlementBus CLI: --host requires a value\./
        );
    });

    await withArgs(['--rpc', '--port'], async () => {
        t.exception(
            () => resolveConfig(),
            /MainSettlementBus CLI: --port requires a value\./
        );
    });
});

test('args: resolveConfig rejects malformed and out-of-range port values', async t => {
    const invalidPorts = ['abc', '123abc', '0', '65536'];

    for (const port of invalidPorts) {
        await withArgs(['--rpc', '--port', port], async () => {
            t.exception(
                () => resolveConfig(),
                /MainSettlementBus CLI: --port must be an integer in range 1-65535\./
            );
        });
    }
});

test('args: reads Pear v3 worker arguments from Bare.argv', t => {
    const args = getArguments({
        Bare: {
            argv: ['bare', 'msb.mjs', '--rpc', '--network', 'testnet']
        }
    });

    t.alike(args, ['--rpc', '--network', 'testnet']);
});
