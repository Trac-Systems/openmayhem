import { MainSettlementBus } from './src/index.js';
import { startRpcServer } from './rpc/rpc_server.js';
import { createConfig, ENV } from './src/config/env.js';

const pearApp = typeof Pear !== 'undefined' ? (Pear.app ?? Pear.config) : undefined;
const runtimeArgs = typeof process !== 'undefined' ? process.argv.slice(2) : [];
const args = pearApp?.args ?? runtimeArgs;
if (args[0] === '--transfer-helper') {
    const { runTransferHelper } = await import('./src/transferHelper.js');
    try {
        const result = await runTransferHelper(args.slice(1));
        console.log(JSON.stringify(result, null, 2));
    } catch (error) {
        console.error(error?.message ?? String(error));
        if (typeof process !== 'undefined') {
            process.exitCode = 1;
        } else {
            throw error;
        }
    }
} else {
const runRpc = args.includes('--rpc');
const storeName = pearApp?.args?.[0] ?? runtimeArgs[0]

const rpc = {
    storeName: pearApp?.args?.[0] ?? runtimeArgs[0],
    enableWallet: false,
    enableInteractiveMode: false
}

const options = args.includes('--rpc') ? rpc : { storeName }
const config = createConfig(ENV.MAINNET, options)
const msb = new MainSettlementBus(config);

msb.ready().then(async () => {
    if (runRpc) {
        console.log('Starting RPC server...');
        const portIndex = args.indexOf('--port');
        const port = (portIndex !== -1 && args[portIndex + 1]) ? parseInt(args[portIndex + 1], 10) : 5000;
        const hostIndex = args.indexOf('--host');
        const host = (hostIndex !== -1 && args[hostIndex + 1]) ? args[hostIndex + 1] : 'localhost';
        startRpcServer(msb, config , host, port);
    } else {
        console.log('RPC server will not be started.');
        msb.interactiveMode();
    }
});
}
