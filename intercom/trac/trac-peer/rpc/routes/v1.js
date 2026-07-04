import {
  handleHealth,
  handleStatus,
  handleGetState,
  handleGetContractSchema,
  handleContractNonce,
  handleContractPrepareTx,
  handleContractTx,
  handleContractFeature,
} from "../handlers.js";

export const v1Routes = [
  { method: "GET", path: "/health", handler: handleHealth },
  { method: "GET", path: "/status", handler: handleStatus },
  { method: "GET", path: "/state", handler: handleGetState },
  { method: "GET", path: "/contract/schema", handler: handleGetContractSchema },
  // Wallet→peer flow: server-side tx prepare + wallet signature + broadcast.
  { method: "GET", path: "/contract/nonce", handler: handleContractNonce },
  { method: "POST", path: "/contract/tx/prepare", handler: handleContractPrepareTx },
  { method: "POST", path: "/contract/tx", handler: handleContractTx },
  { method: "POST", path: "/contract/feature", handler: handleContractFeature },
];
