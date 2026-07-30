import {
  handleHealth,
  handleStatus,
  handleGetState,
  handleGetContractSchema,
  handleContractNonce,
  handleContractTxContext,
  handleContractTx,
} from "../handlers.js";

export const v1Routes = [
  { method: "GET", path: "/health", handler: handleHealth },
  { method: "GET", path: "/status", handler: handleStatus },
  { method: "GET", path: "/state", handler: handleGetState },
  { method: "GET", path: "/contract/schema", handler: handleGetContractSchema },
  { method: "GET", path: "/contract/nonce", handler: handleContractNonce },
  { method: "GET", path: "/contract/tx/context", handler: handleContractTxContext },
  { method: "POST", path: "/contract/tx", handler: handleContractTx },
];
