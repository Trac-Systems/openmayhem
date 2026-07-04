import { buildRequestUrl } from "./utils/url.js";
import { readJsonBody } from "./utils/body.js";
import {
  getStatus,
  getState,
  getStatePrefix,
  getContractSchema,
  contractGenerateNonce,
  contractPrepareTx,
  contractTx,
  contractFeature,
} from "./services.js";

export async function handleHealth({ respond }) {
  respond(200, { ok: true });
}

export async function handleStatus({ respond, peer }) {
  respond(200, await getStatus(peer));
}

export async function handleGetState({ req, respond, peer }) {
  const url = buildRequestUrl(req);
  const key = url.searchParams.get("key");
  const prefix = url.searchParams.get("prefix");
  const confirmed = url.searchParams.get("confirmed");
  const confirmedBool = confirmed == null ? true : confirmed === "true";
  if (prefix != null) {
    const limit = url.searchParams.get("limit");
    const values = await getStatePrefix(peer, prefix, {
      confirmed: confirmedBool,
      limit: limit == null ? 500 : Number(limit),
    });
    return respond(200, { prefix, confirmed: confirmedBool, values });
  }
  const value = await getState(peer, key, { confirmed: confirmedBool });
  respond(200, { key, confirmed: confirmedBool, value });
}

export async function handleGetContractSchema({ respond, peer }) {
  const schema = await getContractSchema(peer);
  respond(200, schema);
}

export async function handleContractNonce({ respond, peer }) {
  const nonce = await contractGenerateNonce(peer);
  respond(200, { nonce });
}

export async function handleContractPrepareTx({ req, respond, peer, maxBodyBytes }) {
  const body = await readJsonBody(req, { maxBytes: maxBodyBytes });
  if (!body || typeof body !== "object") return respond(400, { error: "Missing JSON body." });
  const payload = await contractPrepareTx(peer, {
    prepared_command: body.prepared_command,
    address: body.address,
    nonce: body.nonce,
  });
  respond(200, payload);
}

export async function handleContractTx({ req, respond, peer, maxBodyBytes }) {
  const body = await readJsonBody(req, { maxBytes: maxBodyBytes });
  if (!body || typeof body !== "object") return respond(400, { error: "Missing JSON body." });
  const payload = await contractTx(peer, {
    tx: body.tx,
    prepared_command: body.prepared_command,
    address: body.address,
    signature: body.signature,
    nonce: body.nonce,
    sim: body.sim,
  });
  respond(200, payload);
}

export async function handleContractFeature({ req, respond, peer, maxBodyBytes }) {
  const body = await readJsonBody(req, { maxBytes: maxBodyBytes });
  if (!body || typeof body !== "object") return respond(400, { error: "Missing JSON body." });
  const payload = await contractFeature(peer, {
    feature: body.feature,
    key: body.key,
    value: body.value,
  });
  respond(200, payload);
}
