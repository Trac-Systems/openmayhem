import b4a from "b4a";
import path from "path";
import fs from "fs";
import PeerWallet from "trac-wallet";
import { Peer, Wallet, createConfig as createPeerConfig, ENV as PEER_ENV } from "../src/index.js";
import { MainSettlementBus } from "trac-msb/src/index.js";
import { createConfig as createMsbConfig, ENV as MSB_ENV } from "trac-msb/src/config/env.js"
import { startRpcServer } from "../rpc/rpc_server.js";
import { DEFAULT_RPC_HOST, DEFAULT_RPC_PORT, DEFAULT_MAX_BODY_BYTES } from "../rpc/constants.js";
import { Terminal } from "../src/terminal/index.js";
import { ensureTextCodecs } from "../src/textCodec.js";
import PokemonProtocol from "../dev/pokemonProtocol.js";
import PokemonContract from "../dev/pokemonContract.js";

let process = globalThis.process;
if (globalThis.Pear !== undefined) {
  const { default: bareProcess } = await import("bare-process");
  process = bareProcess;
}

const pearApp = typeof Pear !== "undefined" ? (Pear.app ?? Pear.config) : undefined;
const runtimeArgs = typeof process !== "undefined" ? process.argv.slice(2) : [];
const argv = pearApp?.args ?? runtimeArgs;
const positionalStoreName = argv.find((a) => a !== undefined && !String(a).startsWith("--")) ?? null;

const createMsb = (options) => {
  const config = createMsbConfig(MSB_ENV.MAINNET, options)
  return new MainSettlementBus(config);
}

const toArgMap = (argv) => {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const raw = argv[i];
    if (!raw.startsWith("--")) continue;
    const eq = raw.indexOf("=");
    if (eq !== -1) {
      const k = raw.slice(2, eq);
      const v = raw.slice(eq + 1);
      out[k] = v;
      continue;
    }
    const k = raw.slice(2);
    const next = argv[i + 1];
    if (next !== undefined && !String(next).startsWith("--")) {
      out[k] = next;
      i++;
    } else {
      out[k] = true;
    }
  }
  return out;
};

const ensureTrailingSlash = (value) => (value.endsWith("/") ? value : `${value}/`);

const ensureKeypairFile = async (keyPairPath) => {
  if (fs.existsSync(keyPairPath)) return;
  fs.mkdirSync(path.dirname(keyPairPath), { recursive: true });
  await ensureTextCodecs();
  const wallet = new PeerWallet();
  await wallet.ready;
  if (!wallet.secretKey) {
    await wallet.generateKeyPair();
    await wallet.ready;
  }
  wallet.exportToFile(keyPairPath, b4a.alloc(0));
};

const readHexFile = (filePath, byteLength) => {
  try {
    if (fs.existsSync(filePath)) {
      const hex = fs.readFileSync(filePath, "utf8").trim().toLowerCase();
      if (/^[0-9a-f]+$/.test(hex) && hex.length === byteLength * 2) return hex;
    }
  } catch (_e) {}
  return null;
};

const args = toArgMap(argv);

const rpcEnabled =
  args["rpc"] === true || args["rpc"] === "true" || process.env.PEER_RPC === "true" || process.env.PEER_RPC === "1";
const rpcHost = (args["rpc-host"] && String(args["rpc-host"])) || process.env.PEER_RPC_HOST || DEFAULT_RPC_HOST;
const rpcPortRaw =
  (args["rpc-port"] && String(args["rpc-port"])) || process.env.PEER_RPC_PORT || String(DEFAULT_RPC_PORT);
const rpcPort = parseInt(rpcPortRaw);
if (rpcEnabled && (isNaN(rpcPort) || rpcPort < 1 || rpcPort > 65535)) {
  console.error("Invalid --rpc-port. Expected integer 1-65535.");
  process.exit(1);
}
const rpcAllowOrigin =
  (args["rpc-allow-origin"] && String(args["rpc-allow-origin"])) || process.env.PEER_RPC_ALLOW_ORIGIN || "*";
const rpcMaxBodyBytesRaw =
  (args["rpc-max-body-bytes"] && String(args["rpc-max-body-bytes"])) ||
  process.env.PEER_RPC_MAX_BODY_BYTES ||
  String(DEFAULT_MAX_BODY_BYTES);
const rpcMaxBodyBytes = parseInt(rpcMaxBodyBytesRaw);
if (rpcEnabled && (isNaN(rpcMaxBodyBytes) || rpcMaxBodyBytes < 1)) {
  console.error("Invalid --rpc-max-body-bytes. Expected a positive integer.");
  process.exit(1);
}

const apiTxExposed =
  args["api-tx-exposed"] === true ||
  args["api-tx-exposed"] === "true" ||
  process.env.PEER_API_TX_EXPOSED === "true" ||
  process.env.PEER_API_TX_EXPOSED === "1";

const apiTxExposedEffective = rpcEnabled ? apiTxExposed : false;
if (!rpcEnabled && apiTxExposed) {
  console.warn("Ignoring --api-tx-exposed because RPC is not enabled (use --rpc).");
}

const msbStoresDirectory =
  (args["msb-stores-directory"] && String(args["msb-stores-directory"])) ||
  process.env.MSB_STORES_DIRECTORY ||
  "stores/";

const msbStoreName =
  (args["msb-store-name"] && String(args["msb-store-name"])) ||
  process.env.MSB_STORE_NAME ||
  null;

const msbBootstrap =
  (args["msb-bootstrap"] && String(args["msb-bootstrap"])) ||
  process.env.MSB_BOOTSTRAP ||
  null;

const msbChannel =
  (args["msb-channel"] && String(args["msb-channel"])) ||
  process.env.MSB_CHANNEL ||
  null;

const peerStoresDirectory =
  (args["peer-stores-directory"] && String(args["peer-stores-directory"])) ||
  process.env.PEER_STORES_DIRECTORY ||
  "stores/";

const peerStoreNameRaw =
  (args["peer-store-name"] && String(args["peer-store-name"])) ||
  process.env.PEER_STORE_NAME ||
  (positionalStoreName ? String(positionalStoreName) : null) ||
  "peer";

const subnetBootstrapHex =
  (args["subnet-bootstrap"] && String(args["subnet-bootstrap"])) ||
  process.env.SUBNET_BOOTSTRAP ||
  null;

const subnetChannel =
  (args["subnet-channel"] && String(args["subnet-channel"])) ||
  process.env.SUBNET_CHANNEL ||
  "trac-peer-subnet";

const msbBootstrapHex = msbBootstrap ? String(msbBootstrap).trim().toLowerCase() : null;
if (msbBootstrapHex && !/^[0-9a-f]{64}$/.test(msbBootstrapHex)) {
  console.error(
    `Invalid --msb-bootstrap. Expected 32-byte hex (64 chars), got length ${msbBootstrapHex.length}.`
  );
  process.exit(1);
}

if (!msbBootstrapHex || !msbChannel) {
  console.error(
    "Missing MSB network params. Provide --msb-bootstrap=<hex32> and --msb-channel=<string> (or env MSB_BOOTSTRAP/MSB_CHANNEL)."
  );
  process.exit(1);
}

// Important: this starts a *local MSB node* (its own store) that joins your already-running MSB network.
// trac-peer currently requires an MSB instance to broadcast and to observe confirmed state.

const effectiveMsbStoreName = msbStoreName ?? `${peerStoreNameRaw}-msb`;
const msbStoresFullPath = path.join(ensureTrailingSlash(msbStoresDirectory), effectiveMsbStoreName);
const msbKeyPairPath = path.join(msbStoresFullPath, "db", "keypair.json");
await ensureKeypairFile(msbKeyPairPath);

const peerKeyPairPath = path.join(
  peerStoresDirectory,
  peerStoreNameRaw,
  "db",
  "keypair.json"
);

await ensureKeypairFile(peerKeyPairPath);

const subnetBootstrapFile = path.join(
  peerStoresDirectory,
  peerStoreNameRaw,
  "subnet-bootstrap.hex"
);
let subnetBootstrap = subnetBootstrapHex ? subnetBootstrapHex.trim().toLowerCase() : null;
if (subnetBootstrap) {
  if (!/^[0-9a-f]{64}$/.test(subnetBootstrap)) {
    console.error("Invalid --subnet-bootstrap. Provide 32-byte hex (64 chars).");
    process.exit(1);
  }
  if (subnetBootstrap === msbBootstrap.trim().toLowerCase()) {
    console.error("Subnet bootstrap cannot equal MSB bootstrap.");
    process.exit(1);
  }
} else {
  subnetBootstrap = readHexFile(subnetBootstrapFile, 32);
  if (subnetBootstrap && subnetBootstrap === msbBootstrap.trim().toLowerCase()) {
    console.error("Stored subnet bootstrap equals MSB bootstrap. Delete the file and rerun.");
    process.exit(1);
  }
}

const msb = createMsb({ bootstrap: msbBootstrap, channel: msbChannel, storeName: effectiveMsbStoreName, storesDirectory: msbStoresDirectory})
await msb.ready();

// DevProtocol and DevContract moved to shared src files
const peerConfig = createPeerConfig(PEER_ENV.MAINNET, {
  storesDirectory: ensureTrailingSlash(peerStoresDirectory),
  storeName: peerStoreNameRaw,
  bootstrap: subnetBootstrap ? b4a.from(subnetBootstrap, "hex") : null,
  channel: subnetChannel,
  enableInteractiveMode: rpcEnabled ? false : true,
  apiTxExposed: apiTxExposedEffective,
});

const peer = new Peer({
  config: peerConfig,
  msb,
  wallet: new Wallet(),
  protocol: PokemonProtocol,
  contract: PokemonContract,
});
await peer.ready();

let rpcServer = null;
if (rpcEnabled) {
  rpcServer = startRpcServer(peer, rpcHost, rpcPort, { allowOrigin: rpcAllowOrigin, maxBodyBytes: rpcMaxBodyBytes });
}

const peerMsbAddress = peer.msbClient.pubKeyHexToAddress(peer.wallet.publicKey);
const effectiveSubnetBootstrapHex = peer.base?.key ? b4a.toString(peer.base.key, "hex") : (b4a.isBuffer(peer.config.bootstrap) ? b4a.toString(peer.config.bootstrap, "hex") : String(peer.config.bootstrap));
if (!subnetBootstrap) {
  fs.mkdirSync(path.dirname(subnetBootstrapFile), { recursive: true });
  fs.writeFileSync(subnetBootstrapFile, `${effectiveSubnetBootstrapHex}\n`);
}

console.log("");
console.log("==================== TRAC-PEER RUNNER ====================");
console.log("MSB network bootstrap:", b4a.toString(msb.config.bootstrap, "hex") ?? null);
console.log("MSB channel:", b4a.toString(msb.config.channel, "utf8"));
console.log("MSB wallet address:", msb.wallet?.address ?? null);
console.log("----------------------------------------------------------");
console.log("Peer store:", path.join(peerStoresDirectory, peerStoreNameRaw));
console.log("Peer subnet bootstrap:", effectiveSubnetBootstrapHex);
console.log("Peer subnet channel:", subnetChannel);
console.log("Peer pubkey (hex):", peer.wallet.publicKey);
console.log("Peer MSB address:", peerMsbAddress);
console.log("Subnet writable:", !!peer.base?.writable);
if (peer.base && peer.base.writable === false) {
  console.log("WARNING: Subnet is read-only for this keypair. If you're trying to create a new subnet, delete:");
  console.log("  ", subnetBootstrapFile);
}
console.log("----------------------------------------------------------");
console.log("Next steps:");
console.log("- On your already-running MSB admin node:");
console.log("  - Fund this Peer MSB address (this creates the MSB node entry + covers fee checks):");
console.log("    ", peerMsbAddress);
console.log("- In this peer console:");
console.log("  - /deploy_subnet");
console.log('  - /tx --command "ping hello"');
console.log('  - /get --key "txl"');
console.log("==========================================================");
console.log("");

if (peer.config.enableInteractiveMode) {
  const terminal = new Terminal(peer);
  await terminal.start();
} else {
  console.log("Interactive CLI disabled.");
}

process.on("SIGINT", async () => {
  if (rpcServer) await new Promise((resolve) => rpcServer.close(resolve));
  await Promise.allSettled([peer.close(), msb.close()]);
  process.exit(130);
});
