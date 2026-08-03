import test from 'brittle';
import b4a from 'b4a';

import { createConfig, ENV } from '../../../src/config/env.js';

test('Config: shipped presets still construct successfully', t => {
    const environments = [ENV.MAINNET, ENV.DEVELOPMENT, ENV.TESTNET1];

    for (const environment of environments) {
        const config = createConfig(environment, {});

        t.ok(b4a.isBuffer(config.bootstrap), `${environment}: bootstrap should be normalized to a buffer`);
        t.ok(b4a.isBuffer(config.channel), `${environment}: channel should be normalized to a buffer`);
        t.is(config.bootstrap.length, 32, `${environment}: bootstrap should be 32 bytes`);
        t.is(config.channel.length, 32, `${environment}: channel should be 32 bytes`);
    }
});

test('Config: valid overrideable startup fields remain usable after validation', t => {
    const config = createConfig(ENV.MAINNET, {
        bootstrap: 'ab'.repeat(32),
        channel: 'custom-channel',
        storesDirectory: 'custom-stores/',
        host: '0.0.0.0',
        port: 5050,
        dhtBootstrap: ['node1.example.test:1234', 'node2.example.test:5678']
    });

    t.ok(b4a.isBuffer(config.bootstrap));
    t.is(config.bootstrap.length, 32);
    t.ok(b4a.isBuffer(config.channel));
    t.is(config.channel.length, 32);
    t.is(config.storesDirectory, 'custom-stores');
    t.is(config.host, '0.0.0.0');
    t.is(config.port, 5050);
    t.alike(config.dhtBootstrap, ['node1.example.test:1234', 'node2.example.test:5678']);
});

test('Config: hyperbee cache entry limit defaults and validates', t => {
    const config = createConfig(ENV.MAINNET, {});
    t.is(config.hyperbeeCacheMaxEntries, 65_536);

    const overridden = createConfig(ENV.MAINNET, { hyperbeeCacheMaxEntries: 7 });
    t.is(overridden.hyperbeeCacheMaxEntries, 7);

    t.exception(
        () => createConfig(ENV.MAINNET, { hyperbeeCacheMaxEntries: 0 }),
        /MainSettlementBus Config: hyperbeeCacheMaxEntries must be a positive safe integer\./
    );
});

test('Config: valid buffer bootstrap override remains usable after validation', t => {
    const bootstrap = b4a.alloc(32, 0xab);
    const config = createConfig(ENV.MAINNET, { bootstrap });

    t.ok(b4a.isBuffer(config.bootstrap));
    t.alike(config.bootstrap, bootstrap);
});

test('Config: invalid bootstrap override fails early', t => {
    t.exception(
        () => createConfig(ENV.MAINNET, { bootstrap: 'abc' }),
        /MainSettlementBus Config: bootstrap must be a 32-byte hex string or Buffer\./
    );

    t.exception(
        () => createConfig(ENV.MAINNET, { bootstrap: b4a.alloc(31) }),
        /MainSettlementBus Config: bootstrap must be a 32-byte hex string or Buffer\./
    );
});

test('Config: invalid channel override fails early', t => {
    t.exception(
        () => createConfig(ENV.MAINNET, { channel: '' }),
        /MainSettlementBus Config: channel must be 1-32 bytes long\./
    );

    t.exception(
        () => createConfig(ENV.MAINNET, { channel: 123 }),
        /MainSettlementBus Config: channel must be a string or Buffer\./
    );

    t.exception(
        () => createConfig(ENV.MAINNET, { channel: 'x'.repeat(33) }),
        /MainSettlementBus Config: channel must be 1-32 bytes long\./
    );
});

test('Config: invalid storesDirectory, host, and port overrides fail early', t => {
    t.exception(
        () => createConfig(ENV.MAINNET, { storesDirectory: '' }),
        /MainSettlementBus Config: storesDirectory must be a non-empty string\./
    );

    t.exception(
        () => createConfig(ENV.MAINNET, { storesDirectory: 123 }),
        /MainSettlementBus Config: storesDirectory must be a non-empty string\./
    );

    t.exception(
        () => createConfig(ENV.MAINNET, { host: '' }),
        /MainSettlementBus Config: host must be a non-empty string\./
    );

    t.exception(
        () => createConfig(ENV.MAINNET, { host: 123 }),
        /MainSettlementBus Config: host must be a non-empty string\./
    );

    t.exception(
        () => createConfig(ENV.MAINNET, { port: 0 }),
        /MainSettlementBus Config: port must be an integer in range 1-65535\./
    );

    t.exception(
        () => createConfig(ENV.MAINNET, { port: 65_536 }),
        /MainSettlementBus Config: port must be an integer in range 1-65535\./
    );

    t.exception(
        () => createConfig(ENV.MAINNET, { port: '5000' }),
        /MainSettlementBus Config: port must be an integer in range 1-65535\./
    );
});

test('Config: invalid dhtBootstrap override fails early', t => {
    t.exception(
        () => createConfig(ENV.MAINNET, { dhtBootstrap: 'node1.example.test:1234' }),
        /MainSettlementBus Config: dhtBootstrap must be a non-empty array of strings\./
    );

    t.exception(
        () => createConfig(ENV.MAINNET, { dhtBootstrap: ['node1.example.test:1234', ''] }),
        /MainSettlementBus Config: dhtBootstrap entries must be non-empty strings\./
    );
});
