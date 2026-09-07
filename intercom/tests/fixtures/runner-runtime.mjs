import { getPearRuntime } from '../../trac/trac-peer/src/runnerArgs.js';

const { env, argv, storeLabel, flags } = getPearRuntime();
console.log(JSON.stringify({
  bare: typeof Bare !== 'undefined',
  globalProcess: typeof globalThis.process !== 'undefined',
  checkpoints: env.MAYHEM_WRITER_CHECKPOINTS,
  directory: env.MAYHEM_WRITER_CHECKPOINT_DIR,
  reserve: env.MAYHEM_WRITER_CHECKPOINT_RESERVE_AU,
  argv,
  storeLabel,
  flags,
}));
if (typeof Bare !== 'undefined') Bare.exit(0);
