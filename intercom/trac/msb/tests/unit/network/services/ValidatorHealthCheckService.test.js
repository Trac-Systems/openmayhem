import sinon from 'sinon';
import { test } from 'brittle';
import ValidatorHealthCheckService from '../../../../src/core/network/services/ValidatorHealthCheckService.js';
import { createConfig, ENV } from '../../../../src/config/env.js';
import { EventType } from '../../../../src/utils/constants.js';
import { testKeyPair1, testKeyPair2 } from '../../../fixtures/apply.fixtures.js';

test('ValidatorHealthCheckService', () => {
    test('throws when start is called before ready', async (t) => {
        const config = createConfig(ENV.MAINNET, { validatorHealthCheckInterval: 1000 });
        const service = new ValidatorHealthCheckService(config);

        await t.exception(() => service.start(testKeyPair1.publicKey));
        await service.close();
    });

    test('constructor rejects invalid interval', async (t) => {
        const badConfig = createConfig(ENV.MAINNET, { validatorHealthCheckInterval: -1 });

        await t.exception.all(() => new ValidatorHealthCheckService(badConfig));
    });

    test('constructor defaults interval when missing', async (t) => {
        const config = createConfig(ENV.MAINNET, {});
        const service = new ValidatorHealthCheckService(config);
        await service.ready();

        t.is(service.start(testKeyPair1.publicKey), true);
        t.is(service.start(testKeyPair1.publicKey), false);
        await service.close();
    });

    test('stop returns false when no schedule exists', async (t) => {
        const config = createConfig(ENV.MAINNET, { validatorHealthCheckInterval: 1000 });
        const service = new ValidatorHealthCheckService(config);
        await service.ready();

        t.is(service.stop(testKeyPair1.publicKey), false);
        await service.close();
    });

    test('has throws on invalid public key type', async (t) => {
        const config = createConfig(ENV.MAINNET, { validatorHealthCheckInterval: 1000 });
        const service = new ValidatorHealthCheckService(config);
        await service.ready();

        await t.exception.all(() => service.has(123));
        await service.close();
    });

    test('start returns false when already scheduled', async (t) => {
        const config = createConfig(ENV.MAINNET, { validatorHealthCheckInterval: 1000 });
        const service = new ValidatorHealthCheckService(config);
        await service.ready();

        t.ok(service.start(testKeyPair1.publicKey));
        t.is(service.start(testKeyPair1.publicKey), false);
        await service.close();
    });

    test('emits health check on interval', async (t) => {
        const clock = sinon.useFakeTimers({ now: 0 });
        try {
            const config = createConfig(ENV.MAINNET, { validatorHealthCheckInterval: 1000 });
            const service = new ValidatorHealthCheckService(config);
            await service.ready();

            const emitted = new Promise(resolve => {
                service.once(EventType.VALIDATOR_HEALTH_CHECK, (publicKey, requestId) => {
                    resolve({ publicKey, requestId });
                });
            });
            t.ok(service.start(testKeyPair1.publicKey));

            await clock.tickAsync(1000);
            const { publicKey, requestId } = await emitted;
            t.is(publicKey, testKeyPair1.publicKey.toLowerCase());
            t.ok(requestId);
            await service.close();
        } finally {
            clock.restore();
            sinon.restore();
        }
    });

    test('stopAll cancels scheduled checks', async (t) => {
        const clock = sinon.useFakeTimers({ now: 0 });
        try {
            const config = createConfig(ENV.MAINNET, { validatorHealthCheckInterval: 1000 });
            const service = new ValidatorHealthCheckService(config);
            await service.ready();
            const emitted = [];
            const waitForTwo = new Promise(resolve => {
                service.on(EventType.VALIDATOR_HEALTH_CHECK, (publicKey, requestId) => {
                    emitted.push({ publicKey, requestId });
                    if (emitted.length === 2) resolve();
                });
            });
            t.ok(service.start(testKeyPair1.publicKey));
            t.ok(service.start(testKeyPair2.publicKey));

            await clock.tickAsync(1000);
            await waitForTwo;
            t.is(emitted.length, 2);

            await service.close();

            await clock.tickAsync(1000);
            t.is(emitted.length, 2);
        } finally {
            clock.restore();
            sinon.restore();
        }
    });
});
