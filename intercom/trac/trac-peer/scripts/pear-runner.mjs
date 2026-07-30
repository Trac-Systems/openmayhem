import { spawn, spawnSync } from "node:child_process";
import { constants as osConstants } from "node:os";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { parsePearMajor, selectPearRunnerMode } from "../src/pearCompat.js";

const projectDirectory = fileURLToPath(new URL("../", import.meta.url));
const entrypoint = fileURLToPath(new URL("./run-peer.mjs", import.meta.url));
const appArgs = process.argv.slice(2);

const exitCodeFor = (code, signal) => {
  if (code !== null && code !== undefined) return code;
  const signalNumber = osConstants.signals?.[signal];
  return Number.isInteger(signalNumber) ? 128 + signalNumber : 1;
};

const detectPear = () => {
  for (const args of [
    ["-v"],
    ["-v", "--json"],
  ]) {
    const result = spawnSync("pear", args, {
      cwd: projectDirectory,
      encoding: "utf8",
      windowsHide: true,
    });
    if (result.error || result.status !== 0) continue;
    const output = [result.stdout, result.stderr].filter(Boolean).join("\n");
    if (parsePearMajor(output) !== null) return output;
  }
  return "";
};

const forwardedSignals =
  process.platform === "win32"
    ? ["SIGINT", "SIGBREAK", "SIGTERM"]
    : ["SIGHUP", "SIGINT", "SIGQUIT", "SIGTERM"];

const supervise = (child, { stop, forceStop = null }) => {
  let exited = false;
  let receivedSignal = null;
  let forceTimer = null;

  const signalHandlers = new Map();
  const stopOnHostExit = () => {
    if (!exited) stop("SIGTERM");
  };
  const cleanup = () => {
    if (forceTimer) clearTimeout(forceTimer);
    for (const [signal, handler] of signalHandlers) {
      process.removeListener(signal, handler);
    }
    process.removeListener("exit", stopOnHostExit);
  };
  const requestStop = (signal) => {
    if (receivedSignal !== null) {
      (forceStop ?? stop)(signal);
      return;
    }
    receivedSignal = signal;
    stop(signal);
    if (!forceStop) return;
    forceTimer = setTimeout(() => {
      if (!exited) forceStop();
    }, 5000);
    forceTimer.unref?.();
  };

  for (const signal of forwardedSignals) {
    const handler = () => requestStop(signal);
    signalHandlers.set(signal, handler);
    process.on(signal, handler);
  }
  process.once("exit", stopOnHostExit);

  child.once("error", (error) => {
    exited = true;
    cleanup();
    console.error("Pear runner failed:", error?.message ?? error);
    process.exitCode = 1;
  });
  child.once("exit", (code, signal) => {
    exited = true;
    cleanup();
    if (receivedSignal === null && (signal || (code !== null && code !== 0))) {
      const reason = signal ? `signal ${signal}` : `exit code ${code}`;
      console.error(`Pear runner exited unexpectedly (${reason}).`);
    }
    process.exitCode = receivedSignal
      ? exitCodeFor(null, receivedSignal)
      : exitCodeFor(code, signal);
  });
};

const runLegacy = () => {
  const child = spawn(
    "pear",
    ["run", "-d", "scripts/run-peer.mjs", ...appArgs],
    {
      cwd: projectDirectory,
      stdio: "inherit",
      windowsHide: true,
    }
  );

  supervise(child, {
    stop: (signal) => {
      try {
        child.kill(signal);
      } catch (_e) {}
    },
    forceStop: () => {
      try {
        child.kill("SIGKILL");
      } catch (_e) {}
    },
  });
};

const runModule = async () => {
  const imported = await import("pear-runtime");
  const PearRuntime = imported.default ?? imported;
  const worker = PearRuntime.run(entrypoint, appArgs);

  const onWorkerStdinError = (error) => {
    if (error?.code === "EPIPE" || error?.code === "ERR_STREAM_DESTROYED") return;
    console.error("Pear worker stdin failed:", error?.message ?? error);
  };

  if (worker.stdin) worker.stdin.on("error", onWorkerStdinError);
  if (worker.stdin) process.stdin.pipe(worker.stdin);
  if (worker.stdout) worker.stdout.pipe(process.stdout, { end: false });
  if (worker.stderr) worker.stderr.pipe(process.stderr, { end: false });

  const unpipe = () => {
    if (worker.stdin) process.stdin.unpipe(worker.stdin);
    if (worker.stdin) worker.stdin.removeListener("error", onWorkerStdinError);
    if (worker.stdout) worker.stdout.unpipe(process.stdout);
    if (worker.stderr) worker.stderr.unpipe(process.stderr);
  };

  worker.once("close", unpipe);
  supervise(worker, {
    stop: () => {
      if (!worker.destroyed) worker.destroy();
    },
    forceStop: () => {
      // Sidecar only exposes graceful destroy; use its child handle as a guarded fallback.
      try {
        if (typeof worker._process?.kill === "function") {
          worker._process.kill("SIGKILL");
          return;
        }
      } catch (_e) {}
      if (!worker.destroyed) worker.destroy();
    },
  });
};

const mode = selectPearRunnerMode(detectPear());
if (mode === "legacy") runLegacy();
else await runModule();
