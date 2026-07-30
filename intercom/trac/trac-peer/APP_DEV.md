# trac-peer — App Developer Guide (Protocol + Contract)

This guide explains how to build an “app” on `trac-peer` (a **Protocol + Contract** pair), how wallets/dapps discover what your app supports, and how clients execute app functions via the peer RPC.

For MSB + peer local setup (bootstraps, funding, subnet deployment), see `DOCS.md`.

---

## 1) Mental model (what you’re building)

- **MSB** is the settlement layer. A transaction becomes “real” when MSB accepts it.
- **trac-peer** runs a **subnet** (a smaller P2P network) that derives deterministic state from an ordered log.
- Your **Contract** is executed **locally on every subnet node** from the same ordered ops, so every node derives the same state.
- Your **Protocol** defines:
  - how user input is mapped into typed tx ops: `{ type, value }`
  - optional read/query methods exposed to dApps (via `protocol.api`)

Clients do **not** “call contract functions directly”. They submit a **transaction** (an op), and the contract executes that op during subnet apply.

---

## 2) Where apps live in this repo (today)

The default runner is `scripts/run-peer.mjs`. It currently wires the demo "Tuxemon" app:

- `dev/tuxemonProtocol.js`
- `dev/tuxemonContract.js`

To run your own app locally, simplest workflow:

1) Create `dev/myProtocol.js` and `dev/myContract.js`.
2) Update `scripts/run-peer.mjs` to import your protocol/contract instead of Pokémon.

If you want to keep Pokémon unchanged, add a second runner script (example: `scripts/run-peer-myapp.mjs`) that wires your app.

---

## 3) Contract (state machine)

Your contract class should extend the base contract in `src/artifacts/contract.js` and implement one method per supported operation type.

### 3.1 Registering “ABI-like” metadata (what dApps discover)

Wallets need a machine-readable description of what the contract supports. `trac-peer` exposes this at:

- `GET /v1/contract/schema`

That response is constructed from **contract metadata** if present, and falls back to inference if you register nothing.

The base contract supports two common registration styles:

- `addFunction(name)` — declares that an op exists; value schema is treated as `{}` (untyped).
- `addSchema(name, fastestSchema)` — declares that an op exists and provides an explicit schema for `key`/`value` (preferred).

If you register schemas, wallets/dapps can render forms and validate inputs before signing.

### 3.2 Writing state (recommended key conventions)

Contracts write to subnet state via the storage methods provided by the base contract (Hyperbee-backed).

Recommended:
- Put app state under `app/<appName>/...` so it’s easy to query.
- Do not overwrite reserved system keys (admin/chat/tx indexing, etc).

Example keys:
- `app/tuxedex/<userPubKeyHex>`
- `app/counter/value`

---

## 4) Protocol (command mapping + wallet API)

Your protocol class extends `src/artifacts/protocol.js`.

### 4.1 CLI mapping (`/tx --command "..."`)

The CLI `/tx` starts from a **string**. Your protocol maps that string into a typed operation:

- `mapTxCommand(commandString) -> { type, value } | null`

Examples:
- `"catch"` → `{ type: "catch", value: {} }`
- `"set foo bar"` → `{ type: "set", value: { key: "foo", value: "bar" } }`

### 4.2 dApp-facing API (`protocol.api`)

dApps generally need:
- Read/query methods (get state, derived data)
- A single write path through tx submission (prepare → sign → simulate → broadcast)

In this codebase the base protocol exposes a `ProtocolApi` instance at:
- `protocol.api`

The base protocol also exposes a discovery schema at:
- `protocol.getApiSchema()` (included in `GET /v1/contract/schema`)

If you add read/query methods to `protocol.api` (typically via the protocol’s `extendApi()` pattern), they can be reflected into the RPC schema so dApps know what’s available.

---

## 5) RPC endpoints (what wallets/dapps use)

Run a peer with RPC enabled:

```sh
npm run peer:run -- \
  --msb-bootstrap <hex32> \
  --msb-channel <channel> \
  --rpc \
  --api-tx-exposed \
  --rpc-host 127.0.0.1 \
  --rpc-port 5001
```

Endpoints (all JSON, all under `/v1`):

- `GET /v1/health`
- `GET /v1/status`
- `GET /v1/contract/schema`
- `GET /v1/contract/nonce`
- `GET /v1/contract/tx/context` (returns MSB tx context)
- `POST /v1/contract/tx`
- `GET /v1/state?key=<urlencoded>&confirmed=true|false`

Important notes:
- `--api-tx-exposed` only has effect if you started with `--rpc`.
- Operator/admin actions (deploy subnet, add/remove writers/indexers, chat moderation) are CLI-only and are not exposed by RPC.

---

## 6) Client → peer → contract flow (end-to-end)

This is the “Ethereum-style” flow: a client (typically a dapp/backend) discovers a peer URL, fetches a schema, prepares a tx, requests a wallet signature, then submits it.

### Where the dapp fits (dapp constructs, wallet signs)

- A **dapp** (web/mobile UI) can read: `GET /v1/contract/schema` and `GET /v1/state`.
- For **writes**, the dapp (or a backend the dapp calls) typically:
  1) fetches `nonce` + `tx/context` from the peer,
  2) constructs the tx hash (`tx`) locally,
  3) asks the wallet to **sign** the tx hash,
  4) submits `sim: true` then `sim: false` to the peer.

In other words: the wallet only needs to sign; it does not need to talk to the peer RPC.

### Step A — Discover contract schema

```sh
curl -s http://127.0.0.1:5001/v1/contract/schema | jq
```

Client uses:
- `contract.txTypes` (what tx types exist)
- `contract.ops[type]` (input structure for each type, when available)
- `api.methods` (optional read/query methods exposed by the protocol api)

### Step B — Get a nonce (client)

```sh
curl -s http://127.0.0.1:5001/v1/contract/nonce | jq
```

### Step C — Get tx context + build tx hash (client)

The client constructs a typed command (this is app-specific):

```json
{ "type": "catch", "value": {} }
```

Then the client asks the peer for the MSB tx context (no computation):

```sh
curl -s http://127.0.0.1:5001/v1/contract/tx/context | jq
```

The response contains an `msb` object with the fields the client needs to build the tx preimage:
- `networkId`
- `txv`
- `iw` (peer writer key)
- `bs` (subnet bootstrap)
- `mbs` (MSB bootstrap)
- `operationType` (currently `12`)

From there, the client computes locally:
- `command_hash = blake3(JSON.stringify(prepared_command))` (hex32)
- `tx = blake3(createMessage(networkId, txv, iw, command_hash, bs, mbs, nonce, operationType))` (hex32)

### Step D — Sign locally with the wallet

Wallet signs the **bytes** of `tx` (32 bytes) with its private key to produce:
- `signature` (hex64)

### Step E — Simulate (recommended)

```sh
curl -s -X POST http://127.0.0.1:5001/v1/contract/tx \
  -H 'Content-Type: application/json' \
  -d '{
    "tx": "<tx-hex32>",
    "prepared_command": { "type": "catch", "value": {} },
    "address": "<wallet-pubkey-hex32>",
    "signature": "<signature-hex64>",
    "nonce": "<nonce-hex32>",
    "sim": true
  }' | jq
```

Simulation runs the same MSB-level validations the real tx will face (fee balance, signature, bootstrap checks, etc.) and then executes the contract against an in-memory storage view.

### Step F — Broadcast (real tx)

```sh
curl -s -X POST http://127.0.0.1:5001/v1/contract/tx \
  -H 'Content-Type: application/json' \
  -d '{
    "tx": "<tx-hex32>",
    "prepared_command": { "type": "catch", "value": {} },
    "address": "<wallet-pubkey-hex32>",
    "signature": "<signature-hex64>",
    "nonce": "<nonce-hex32>",
    "sim": false
  }' | jq
```

### Step G — Read app state

Apps typically write under `app/...` (app-defined). Read via:

```sh
curl -s 'http://127.0.0.1:5001/v1/state?key=<urlencoded-hyperbee-key>&confirmed=false' | jq
```

Example (Tuxemon demo app):

```sh
curl -s 'http://127.0.0.1:5001/v1/state?key=app%2Ftuxedex%2F<wallet-pubkey-hex32>&confirmed=false' | jq
```
```

The `confirmed` flag controls whether you read from:
- the latest local view (`confirmed=false`), or
- the signed/confirmed view (`confirmed=true`)

---

## 7) Minimal app skeleton (example)

### Contract

```js
export default class MyContract extends Contract {
  constructor (...args) {
    super(...args);
    this.addSchema("inc", { value: { $$type: "object", by: { type: "number", integer: true, min: 1, max: 100 } } });
  }

  async inc (op) {
    const by = op?.value?.value?.by ?? 1;
    const current = (await this.get("app/counter/value"))?.value ?? 0;
    await this.put("app/counter/value", current + by);
  }
}
```

### Protocol

```js
export default class MyProtocol extends Protocol {
  mapTxCommand (command) {
    if (command === "inc") return { type: "inc", value: { by: 1 } };
    return null;
  }
}
```

Wire it in the runner (`scripts/run-peer.mjs`) by importing your classes.
