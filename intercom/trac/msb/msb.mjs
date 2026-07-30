import { startRpcServer } from './rpc/rpc_server.js';
import { startInteractiveMode } from './cli/interactive.js';
import { isRpcEnabled, resolveConfig } from './src/config/args.js';

const config = resolveConfig()

const start = async () => {
    if (isRpcEnabled()) {
        await startRpcServer(config)
    } else {
        await startInteractiveMode(config)
    }
}

start()
