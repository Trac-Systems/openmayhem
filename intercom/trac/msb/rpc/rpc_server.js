import { createServer } from "./create_server.js";

// Called by msb.mjs file
export function startRpcServer(msbInstance, config ,host, port) {
    const server = createServer(msbInstance, config)

    return server.listen(port, host, () => {
        console.log(`Running RPC with http at http://${host}:${port}`);
    });
}
