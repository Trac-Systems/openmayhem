import { createServer } from "./create_server.js";
import { MainSettlementBus } from "../src/index.js";

// Called by msb.mjs file
export async function startRpcServer(config) {
    console.log('Starting RPC server...');
    const msb = new MainSettlementBus(config);
    await msb.ready()
    const server = createServer(msb, config)

    return server.listen(config.port, config.host, () => {
        console.log(`Running RPC with http at http://${config.host}:${config.port}`);
    });
}
