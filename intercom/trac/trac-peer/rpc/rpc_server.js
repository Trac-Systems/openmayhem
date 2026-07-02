import { createServer } from "./create_server.js";

export function startRpcServer(peer, host, port, options = {}) {
  const server = createServer(peer, options);
  return server.listen(port, host, () => {
    console.log(`trac-peer RPC listening at http://${host}:${port}`);
  });
}

