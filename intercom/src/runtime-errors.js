const errorText = (error) => error?.stack || error?.message || String(error);

export const installFatalRuntimeErrorPolicy = (
  runtime,
  { log = (...args) => console.error(...args) } = {}
) => {
  let exiting = false;
  const fatal = (kind, error) => {
    if (exiting) return;
    exiting = true;
    log(`[intercom] fatal ${kind}; process supervisor restart required:`, errorText(error));
    if (runtime) runtime.exitCode = 1;
    if (typeof runtime?.exit === 'function') runtime.exit(1);
  };

  if (typeof runtime?.once === 'function') {
    runtime.once('uncaughtException', (error) => fatal('uncaught exception', error));
    runtime.once('unhandledRejection', (error) => fatal('unhandled rejection', error));
  }
  return fatal;
};

export default installFatalRuntimeErrorPolicy;
