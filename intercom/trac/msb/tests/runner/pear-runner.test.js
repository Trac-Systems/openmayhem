import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import test from 'node:test';

import {
    detectPearMajor,
    parsePearMajor,
    run,
    runWithPearV2,
    runWithPearV3
} from '../../scripts/pear-runner.js';

test('parses the Pear runtime SemVer reported by pear -v', () => {
    const pearV2 = [
        'v0.3243.key / v2.6.5',
        'Key=key',
        'Fork=0',
        'Length=3243',
        'SemVer=2.6.5'
    ].join('\n');

    assert.equal(parsePearMajor(pearV2), 2);
    assert.equal(parsePearMajor('v0.4000.key / v3.0.0'), 3);
    assert.equal(parsePearMajor('unrecognized output'), null);
});

test('treats a missing Pear CLI as the module-based v3 path', () => {
    const runCommand = () => ({
        error: Object.assign(new Error('not found'), { code: 'ENOENT' })
    });

    assert.equal(detectPearMajor(runCommand), null);
});

test('detects the installed Pear major from pear -v output', () => {
    const runCommand = () => ({
        status: 0,
        stdout: 'v0.4000.key / v3.0.0\nSemVer=3.0.0',
        stderr: ''
    });

    assert.equal(detectPearMajor(runCommand), 3);
});

test('treats successful unrecognized Pear CLI output as the module-based v3 path', () => {
    const runCommand = () => ({
        status: 0,
        stdout: '',
        stderr: ''
    });

    assert.equal(detectPearMajor(runCommand), null);
});

test('selects pear run for v2 and the module-based runner for v3', async () => {
    const calls = [];
    const runners = {
        runPearV2: async args => {
            calls.push(['v2', args]);
            return 2;
        },
        runPearV3: async args => {
            calls.push(['v3', args]);
            return 3;
        }
    };

    assert.equal(await run(['--network', 'mainnet'], {
        detectVersion: () => 2,
        ...runners
    }), 2);
    assert.equal(await run(['--network', 'testnet'], {
        detectVersion: () => 3,
        ...runners
    }), 3);
    assert.equal(await run(['--network', 'development'], {
        detectVersion: () => null,
        ...runners
    }), 3);
    assert.deepEqual(calls, [
        ['v2', ['--network', 'mainnet']],
        ['v3', ['--network', 'testnet']],
        ['v3', ['--network', 'development']]
    ]);
});

test('Pear v2 runner invokes pear run and forwards application arguments', async () => {
    const child = new EventEmitter();
    let invocation;
    const spawnProcess = (command, args, options) => {
        invocation = { command, args, options };
        setImmediate(() => child.emit('exit', 0, null));
        return child;
    };

    assert.equal(await runWithPearV2(['--rpc', '--port', '5000'], spawnProcess), 0);
    assert.equal(invocation.command, 'pear');
    assert.deepEqual(invocation.args, ['run', '.', '--rpc', '--port', '5000']);
    assert.equal(invocation.options.stdio, 'inherit');
});

test('Pear v3 runner starts the MSB Bare worker and forwards stdio', async () => {
    const hostProcess = new EventEmitter();
    hostProcess.stdin = new PassThrough();
    hostProcess.stdout = new PassThrough();
    hostProcess.stderr = new PassThrough();

    const worker = new EventEmitter();
    worker.stdin = new PassThrough();
    worker.stdout = new PassThrough();
    worker.stderr = new PassThrough();
    worker.destroy = () => {};

    let invocation;
    const PearRuntime = {
        run: (entrypoint, args) => {
            invocation = { entrypoint, args };
            setImmediate(() => worker.emit('exit', 7, null));
            return worker;
        }
    };

    const exitCode = await runWithPearV3(['--network', 'testnet'], {
        loadPearRuntime: async () => ({ default: PearRuntime }),
        hostProcess
    });

    assert.equal(exitCode, 7);
    assert.match(invocation.entrypoint, /msb\.mjs$/);
    assert.deepEqual(invocation.args, ['--network', 'testnet']);
    assert.equal(hostProcess.listenerCount('SIGINT'), 0);
    assert.equal(hostProcess.listenerCount('SIGTERM'), 0);
});

test('Pear v3 runner terminates the worker with the host signal exit code', async () => {
    const hostProcess = new EventEmitter();
    const worker = new EventEmitter();
    let destroyed = false;

    worker.destroy = () => {
        destroyed = true;
        setImmediate(() => worker.emit('exit', null, 'SIGTERM'));
    };

    const exitCodePromise = runWithPearV3([], {
        loadPearRuntime: async () => ({
            default: { run: () => worker }
        }),
        hostProcess
    });

    setImmediate(() => hostProcess.emit('SIGINT'));

    assert.equal(await exitCodePromise, 130);
    assert.equal(destroyed, true);
});
