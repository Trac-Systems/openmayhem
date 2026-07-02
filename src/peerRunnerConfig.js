import fs from "fs";
import path from "path";
import { ensureTrailingSlash } from "./runnerArgs.js";

const readHexFile = (filePath, byteLength) => {
  try {
    if (fs.existsSync(filePath)) {
      const hex = fs.readFileSync(filePath, "utf8").trim().toLowerCase();
      if (/^[0-9a-f]+$/.test(hex) && hex.length === byteLength * 2) return hex;
    }
  } catch (_e) {}
  return null;
};

export const resolvePeerRunnerConfig = ({ env = {}, storeLabel = null, flags = {} }) => {
  const msbStoresDirectory = ensureTrailingSlash(
    (flags["msb-stores-directory"] && String(flags["msb-stores-directory"])) ||
      env.MSB_STORES_DIRECTORY ||
      "stores/"
  );

  const peerStoresDirectory = ensureTrailingSlash(
    (flags["peer-stores-directory"] && String(flags["peer-stores-directory"])) ||
      env.PEER_STORES_DIRECTORY ||
      "stores/"
  );

  const peerStoreNameRaw =
    (flags["peer-store-name"] && String(flags["peer-store-name"])) ||
    env.PEER_STORE_NAME ||
    storeLabel ||
    "peer";
  const peerStoreName = ensureTrailingSlash(peerStoreNameRaw);

  const msbStoreName =
    (flags["msb-store-name"] && String(flags["msb-store-name"])) ||
    env.MSB_STORE_NAME ||
    `${peerStoreNameRaw}-msb`;

  const msbBootstrap =
    (flags["msb-bootstrap"] && String(flags["msb-bootstrap"])) || env.MSB_BOOTSTRAP || null;
  const msbChannel = (flags["msb-channel"] && String(flags["msb-channel"])) || env.MSB_CHANNEL || null;

  const msbBootstrapHex = msbBootstrap ? String(msbBootstrap).trim().toLowerCase() : null;
  if (msbBootstrapHex && !/^[0-9a-f]{64}$/.test(msbBootstrapHex)) {
    throw new Error(
      `Invalid --msb-bootstrap. Expected 32-byte hex (64 chars), got length ${msbBootstrapHex.length}.`
    );
  }
  if (!msbBootstrapHex || !msbChannel) {
    throw new Error(
      "Missing MSB network params. Provide --msb-bootstrap=<hex32> and --msb-channel=<string> (or env MSB_BOOTSTRAP/MSB_CHANNEL)."
    );
  }

  const subnetChannel =
    (flags["subnet-channel"] && String(flags["subnet-channel"])) ||
    env.SUBNET_CHANNEL ||
    "trac-peer-subnet";

  const subnetBootstrapHex =
    (flags["subnet-bootstrap"] && String(flags["subnet-bootstrap"])) || env.SUBNET_BOOTSTRAP || null;

  const subnetBootstrapFile = path.join(peerStoresDirectory, peerStoreNameRaw, "subnet-bootstrap.hex");

  let subnetBootstrap = subnetBootstrapHex ? subnetBootstrapHex.trim().toLowerCase() : null;
  if (subnetBootstrap) {
    if (!/^[0-9a-f]{64}$/.test(subnetBootstrap)) {
      throw new Error("Invalid --subnet-bootstrap. Provide 32-byte hex (64 chars).");
    }
    if (subnetBootstrap === msbBootstrapHex) {
      throw new Error("Subnet bootstrap cannot equal MSB bootstrap.");
    }
  } else {
    subnetBootstrap = readHexFile(subnetBootstrapFile, 32);
    if (subnetBootstrap && subnetBootstrap === msbBootstrapHex) {
      throw new Error("Stored subnet bootstrap equals MSB bootstrap. Delete the file and rerun.");
    }
  }

  const msbStoresFullPath = `${msbStoresDirectory}/${msbStoreName}`;
  const msbKeyPairPath = `${msbStoresFullPath}/db/keypair.json`;

  const peerKeyPairPath = path.join(peerStoresDirectory, peerStoreName, "db", "keypair.json");

  const rpcEnabled = flags.rpc === true || flags.rpc === "true" || env.PEER_RPC === "true" || env.PEER_RPC === "1";
  const rpcHost =
    (flags["rpc-host"] && String(flags["rpc-host"])) || env.PEER_RPC_HOST || "127.0.0.1";
  const rpcPortRaw = (flags["rpc-port"] && String(flags["rpc-port"])) || env.PEER_RPC_PORT || "5001";
  const rpcPort = parseInt(rpcPortRaw);
  if (isNaN(rpcPort) || rpcPort < 1 || rpcPort > 65535) {
    throw new Error("Invalid --rpc-port. Expected integer 1-65535.");
  }
  const rpcAllowOrigin = (flags["rpc-allow-origin"] && String(flags["rpc-allow-origin"])) || env.PEER_RPC_ALLOW_ORIGIN || "*";
  const rpcMaxBodyBytesRaw =
    (flags["rpc-max-body-bytes"] && String(flags["rpc-max-body-bytes"])) || env.PEER_RPC_MAX_BODY_BYTES || "1000000";
  const rpcMaxBodyBytes = parseInt(rpcMaxBodyBytesRaw);
  if (isNaN(rpcMaxBodyBytes) || rpcMaxBodyBytes < 1) {
    throw new Error("Invalid --rpc-max-body-bytes. Expected a positive integer.");
  }

  return {
    msbStoresDirectory,
    peerStoresDirectory,
    peerStoreNameRaw,
    peerStoreName,
    msbStoreName,
    msbBootstrapHex,
    msbChannel,
    subnetChannel,
    subnetBootstrap,
    subnetBootstrapFile,
    msbKeyPairPath,
    peerKeyPairPath,
    rpcEnabled,
    rpcHost,
    rpcPort,
    rpcAllowOrigin,
    rpcMaxBodyBytes,
  };
};
