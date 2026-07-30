import { spawn, spawnSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SIGNAL_EXIT_CODES = new Map([
    ['SIGHUP', 129],
    ['SIGINT', 130],
    ['SIGQUIT', 131],
    ['SIGTERM', 143]
]);

const projectDirectory = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const workerEntrypoint = path.join(projectDirectory, 'msb.mjs');

export const parsePearMajor = output => {
    const semver = output.match(/\bSemVer\s*=\s*(\d+)\.\d+\.\d+\b/i);
    if (semver) {
        return Number(semver[1]);
    }

    const summary = output.match(/\/\s*v(\d+)\.\d+\.\d+\b/i);
    return summary ? Number(summary[1]) : null;
};

export const detectPearMajor = (runCommand = spawnSync) => {
    const result = runCommand('pear', ['-v'], {
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'pipe']
    });

    if (result.error?.code === 'ENOENT') {
        return null;
    }

    if (result.error) {
        throw new Error(`Could not check the installed Pear version: ${result.error.message}`);
    }

    const output = `${result.stdout ?? ''}\n${result.stderr ?? ''}`;

    if (result.status !== 0) {
        throw new Error('Could not determine the installed Pear major version from `pear -v`.');
    }

    return parsePearMajor(output);
};

const exitCodeFor = (code, signal) => {
    if (Number.isInteger(code)) {
        return code;
    }

    return SIGNAL_EXIT_CODES.get(signal) ?? 1;
};

export const runWithPearV2 = (
    args,
    spawnProcess = spawn
) => new Promise((resolve, reject) => {
    const child = spawnProcess('pear', ['run', '.', ...args], {
        cwd: projectDirectory,
        env: process.env,
        stdio: 'inherit'
    });

    child.once('error', reject);
    child.once('exit', (code, signal) => resolve(exitCodeFor(code, signal)));
});

const pipeWorkerStdio = (worker, hostProcess) => {
    if (worker.stdin && hostProcess.stdin?.pipe) {
        hostProcess.stdin.pipe(worker.stdin);
    }
    if (worker.stdout?.pipe && hostProcess.stdout) {
        worker.stdout.pipe(hostProcess.stdout);
    }
    if (worker.stderr?.pipe && hostProcess.stderr) {
        worker.stderr.pipe(hostProcess.stderr);
    }
};

export const runWithPearV3 = async (
    args,
    {
        loadPearRuntime = () => import('pear-runtime'),
        hostProcess = process
    } = {}
) => {
    let PearRuntime;

    try {
        const pearRuntimeModule = await loadPearRuntime();
        PearRuntime = pearRuntimeModule.default ?? pearRuntimeModule;
    } catch (error) {
        throw new Error(
            'Pear v3 requires the pear-runtime dependency. Run npm install before starting MSB.',
            { cause: error }
        );
    }

    const worker = PearRuntime.run(workerEntrypoint, args);
    pipeWorkerStdio(worker, hostProcess);

    return new Promise((resolve, reject) => {
        let requestedSignal = null;
        const signalListeners = [];

        const cleanup = () => {
            for (const [signal, listener] of signalListeners) {
                hostProcess.removeListener(signal, listener);
            }

            if (worker.stdin && hostProcess.stdin?.unpipe) {
                hostProcess.stdin.unpipe(worker.stdin);
            }
        };

        for (const signal of SIGNAL_EXIT_CODES.keys()) {
            const listener = () => {
                if (requestedSignal !== null) {
                    return;
                }

                requestedSignal = signal;
                worker.destroy();
            };

            signalListeners.push([signal, listener]);
            hostProcess.once(signal, listener);
        }

        worker.once('error', error => {
            cleanup();
            reject(error);
        });
        worker.once('exit', (code, signal) => {
            cleanup();
            resolve(exitCodeFor(code, requestedSignal ?? signal));
        });
    });
};

export const run = async (
    args = process.argv.slice(2),
    {
        detectVersion = detectPearMajor,
        runPearV2 = runWithPearV2,
        runPearV3 = runWithPearV3
    } = {}
) => {
    const major = detectVersion();

    if (major !== null && major < 3) {
        return runPearV2(args);
    }

    return runPearV3(args);
};

const isMainModule = process.argv[1] &&
    path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);

if (isMainModule) {
    try {
        process.exitCode = await run();
    } catch (error) {
        console.error(`MainSettlementBus Pear runner: ${error.message}`);
        process.exitCode = 1;
    }
}
