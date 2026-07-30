// This runner is intentionally small (mirrors MSB's brittle runner style).
import test from 'brittle';

test.pause();
await import('./applyGuards.test.js');
await import('./baseContractProtocol.test.js');
await import('./cliTx.test.js');
await import('./operations.test.js');
await import('./simFunds.test.js');
await import('./msbTxValidation.test.js');
await import('./walletNetworkConfig.test.js');
await import('./pearCompat.test.js');
await import('./terminalRuntime.test.js');
test.resume();
