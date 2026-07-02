# trac-peer — Setup & App Guide

This document explains how to run `trac-peer`, connect it to an existing MSB network, create/join a subnet, and test the built‑in demo app (“contract”) (`ping` + `set`), plus the HTTP RPC API used by wallets/apps.

It’s written to be usable even if you’re not deeply familiar with P2P/blockchain systems.

If something doesn’t work, jump to **Troubleshooting** at the end.

---

## What you are running (plain English)

- **MSB (Main Settlement Bus)** is the settlement layer. A transaction becomes “real” when MSB accepts it (validators confirm it).
- **trac-peer** is a subnet runner. A **subnet** is a smaller P2P network with:
  - a shared ordered log (who wrote what, in what order)
  - a deterministic app/contract state derived from that log
- Subnet nodes only **execute** a transaction locally once they can prove it exists in MSB (confirmed).

In practice:
1) You run an MSB network (you already have this).
2) You run one or more `trac-peer` nodes that join that MSB network (each trac-peer starts a small “MSB client node” in-process so it can talk to MSB).
3) Those peers form (or join) a subnet and replicate state P2P.
4) When you call `/tx`, trac-peer broadcasts the tx to MSB.
5) When MSB confirms it, the subnet appends an op referencing that MSB tx and all peers execute the same contract logic and derive the same state.

---

## Requirements

- Node.js + npm (recommended: a modern Node LTS).
- `pear` is optional (you can run in pure Node). If you use Pear, add it to PATH as Pear suggests.
- You need your **MSB bootstrap** (32‑byte hex / 64 hex characters) and **MSB channel** (string).

---

## Install

From the `trac-peer` repo folder:

```sh
npm install
```

---

## Key concepts you will see in logs

- **Store**: a local folder where a node saves its data (keys, logs, derived state). In this repo it defaults to `stores/<name>/...`.
- **MSB bootstrap**: the unique ID of the MSB network you are joining (32 bytes, hex string length 64).
- **MSB channel**: a discovery topic used to find the MSB network peers (must match your MSB network).
- **Subnet bootstrap**: the unique ID of a subnet (32 bytes, hex string length 64). Everyone in the same subnet must use the same subnet bootstrap.
- **Subnet channel**: a discovery topic used to find other subnet peers. Everyone in the same subnet must use the same subnet channel.
- **Peer pubkey (hex)**: your trac-peer identity key (public). Used for subnet permissions (admin).
- **Peer writer key (hex)**: the key that identifies a subnet writer (used for add/remove writer/indexer).
- **Peer MSB address**: the MSB “account/address” derived from your peer keypair. MSB must fund it so MSB fee checks pass.

Important safety note:
- Do **not** share your `Secret Key` printed by `/get_keys`.

---

## TL;DR (first time, one peer)

1) Start peer:

```sh
npm run peer:run -- --msb-bootstrap=<hex32> --msb-channel=<channel>
```

2) Copy the printed `Peer MSB address: trac1...` and fund it on your MSB admin node.
3) In the peer console:

```txt
/deploy_subnet
/add_admin --address <your-peer-publicKey-hex>
/tx --command "ping hello"
/get --key app/ping/<tx-hash> --confirmed false
```

---

## Start a peer (Node runner)

The Node runner starts:
- a local MSB client node (in-process) that joins your MSB network (new store), and
- the `trac-peer` subnet node on top of it.

Basic:

```sh
npm run peer:run -- \
  --msb-bootstrap=<32-byte-hex> \
  --msb-channel=<channel-string>
```

If you want separate store names (recommended when running multiple peers on one machine):

```sh
npm run peer:run -- \
  --msb-bootstrap=<32-byte-hex> \
  --msb-channel=<channel-string> \
  --msb-store-name=peer-msb-1 \
  --peer-store-name=peer1
```

Notes:
- In `zsh`, always put all flags **on the same command** or use `\` line continuations. If you start a new line without `\`, your shell will treat it as a new command.

---

## Start a peer (Pear runner)

Pear runner is similar, but uses Pear runtime (like MSB does).

Recommended: set store names explicitly (this avoids confusion when running multiple nodes on one machine).

```sh
npm run peer:pear -- \
  --msb-bootstrap=<32-byte-hex> \
  --msb-channel=<channel-string> \
  --msb-store-name=peer-msb-1 \
  --peer-store-name=peer1
```

Notes:
- If `--peer-store-name` is omitted, you *can* pass an optional first positional arg (Pear “store label”) and it will be used as the peer store name.
- To start a fresh subnet, delete `stores/<peer-store-name>/subnet-bootstrap.hex` (and optionally the whole store folder).

---

## First-time setup: fund your peer on MSB

After a peer starts, it prints something like:

- `Peer MSB address: trac1...`

On your already-running MSB admin node, **fund that address**.

Why?
- MSB validators reject txs from addresses that don’t exist in MSB state or that have insufficient balance to pay fees.

Until the address is funded, you’ll see errors like:
- `Requester address not found in state`

---

## Register the subnet in MSB (required before TX settlement)

In the peer console:

```txt
/deploy_subnet
```

This registers the subnet bootstrap + subnet channel with MSB so TX settlement is allowed.

---

## Become subnet admin (recommended)

Admin is stored in subnet state (`admin` key). Admin can manage writers/indexers and chat moderation.

In the peer console:

```txt
/add_admin --address <your-peer-publicKey-hex>
```

You can find your public key:
- printed on startup as `Peer pubkey (hex): ...`, or
- via `/get_keys` (public key only; never share secret key).

Verify:

```txt
/get --key admin
```

Notes:
- `/add_admin` is allowed only once, and only from the **bootstrap node**.

---

## Test the demo app (“contract”) manually

The default runner includes a demo protocol + demo contract:
- `ping` writes `app/ping/<txHash>`
- `set` writes `app/kv/<key>`

### 1) Send a ping tx

```txt
/tx --command "ping hello"
```

You should see `MSB TX broadcasted: <hash>` and then after confirmation an `appended...` message.

Read what the contract wrote:

```txt
/get --key app/ping/<tx-hash>
```

### 2) Set and read a key/value

```txt
/tx --command "set foo bar"
/get --key app/kv/foo
```

You should see `value: "bar"` plus metadata (tx hash, from).

---

## Run a 3-node subnet (admin + writer + reader)

You’ll run three terminals. The important rule is: **each node must use a different store**, but all nodes must point to the same **MSB bootstrap/channel** and the same **subnet bootstrap/channel**.

### Terminal 1: start the bootstrap/admin node (Peer 1)

```sh
npm run peer:run -- \
  --msb-bootstrap=<msb-bootstrap-hex32> \
  --msb-channel=<msb-channel> \
  --msb-store-name=peer-msb-1 \
  --peer-store-name=peer1
```

Then:

1) Fund the printed `Peer MSB address: trac1...` on your MSB admin node.
2) In Peer 1 console:

```txt
/deploy_subnet
/add_admin --address <peer1-publicKey-hex>
/set_chat_status --enabled 1
/set_nick --nick "admin"
```

### Terminal 2: start a second node that will become a writer (Peer 2)

```sh
npm run peer:run -- \
  --msb-bootstrap=<msb-bootstrap-hex32> \
  --msb-channel=<msb-channel> \
  --msb-store-name=peer-msb-2 \
  --peer-store-name=peer2 \
  --subnet-bootstrap=<subnet-bootstrap-hex32> \
  --subnet-channel=trac-peer-subnet
```

Peer 2 will print:
- `Peer Writer: <peer2-writerKey-hex>`
- `Peer pubkey (hex): <peer2-publicKey-hex>`
- `Peer MSB address: trac1...` (fund it on MSB too, otherwise Peer 2 cannot settle `/tx`)

On Peer 1 (admin), add Peer 2 as writer (and optionally indexer):

```txt
/add_writer --key <peer2-writerKey-hex>
/add_indexer --key <peer2-writerKey-hex>
```

### Terminal 3: start a read-only node (Peer 3)

Start Peer 3 exactly like Peer 2 but do not add its writer key as a writer:

```sh
npm run peer:run -- \
  --msb-bootstrap=<msb-bootstrap-hex32> \
  --msb-channel=<msb-channel> \
  --msb-store-name=peer-msb-3 \
  --peer-store-name=peer3 \
  --subnet-bootstrap=<subnet-bootstrap-hex32> \
  --subnet-channel=trac-peer-subnet
```

Peer 3 should replicate state (you’ll see connections in `/stats`) but it won’t be able to append `/tx` unless an admin adds its writer key.

### Test: chat + tx across peers

1) On Peer 2:

```txt
/set_nick --nick "writer2"
/post --message "hello from peer2"
```

2) On Peer 1 (admin), send a tx:

```txt
/tx --command "set shared hello-world"
```

3) On Peer 3 (reader), read the state:

```txt
/get --key app/kv/shared
```

---

## Add more peers (join an existing subnet)

To simulate multiple nodes, run multiple peers with different stores.

### Peer 1 (already running)
Peer 1 prints the subnet bootstrap (or stores it to `stores/<peer-store>/subnet-bootstrap.hex`).

### Peer 2 (join the same subnet)
Use the same subnet bootstrap and channel:

```sh
npm run peer:run -- \
  --msb-bootstrap=<msb-bootstrap-hex32> \
  --msb-channel=<msb-channel> \
  --msb-store-name=peer-msb-2 \
  --peer-store-name=peer2 \
  --subnet-bootstrap=<subnet-bootstrap-hex32> \
  --subnet-channel=trac-peer-subnet
```

If you started Peer 1 with Pear, you can also start Peer 2 with Pear:

```sh
npm run peer:pear -- \
  --msb-bootstrap=<msb-bootstrap-hex32> \
  --msb-channel=<msb-channel> \
  --msb-store-name=peer-msb-2 \
  --peer-store-name=peer2 \
  --subnet-bootstrap=<subnet-bootstrap-hex32> \
  --subnet-channel=trac-peer-subnet
```

### Writers vs readers
- A peer is **writer** when it has permission to append to the subnet log.
- A peer is **reader** when it can replicate but not append.

Admin can add a writer by writer key:

```txt
/add_writer --key <peer2-writerKey-hex>
```

You can see a peer’s writer key in its startup banner: `Peer Writer: ...`

---

## Chat (in-subnet)

Chat is disabled by default.

1) Enable chat (admin-only):

```txt
/set_chat_status --enabled 1
```

2) Set a nickname:

```txt
/set_nick --nick "alice"
```

3) Post a message:

```txt
/post --message "Hello from peer1"
```

Messages are replicated like any other subnet op.

---

## HTTP RPC (wallet/app API)

RPC is an HTTP server that runs alongside your peer and lets a wallet/app connect via URL (Ethereum-style).

Important: operator/admin controls (deploy subnet, writer/indexer management, chat moderation) are **CLI-only** and are not exposed via RPC.

### Start with RPC enabled (Node)

```sh
npm run peer:run-rpc -- \
  --msb-bootstrap=<hex32> \
  --msb-channel=<channel> \
  --api-tx-exposed \
  --rpc-host=127.0.0.1 \
  --rpc-port=5001
```

### Start with RPC enabled (Pear)

```sh
npm run peer:pear-rpc -- \
  --msb-bootstrap=<hex32> \
  --msb-channel=<channel> \
  --msb-store-name=peer-msb-rpc \
  --peer-store-name=peer-rpc \
  --api-tx-exposed \
  --rpc-host=127.0.0.1 \
  --rpc-port=5001
```

### Important: the health endpoint is versioned

- ✅ `GET http://127.0.0.1:5001/v1/health`
- ❌ `GET http://127.0.0.1:5001/health` (returns 404)

### Common RPC calls

- Status:
  - `GET /v1/status`
- Contract schema (JSON Schema; contract tx types + Protocol API methods):
  - `GET /v1/contract/schema`
- Read state:
  - `GET /v1/state?key=app%2Fkv%2Ffoo&confirmed=true`
- Wallet tx flow:
  - `GET /v1/contract/nonce`
  - `POST /v1/contract/tx/prepare` body: `{ "prepared_command": { "type": "...", "value": {} }, "address": "<pubkey-hex32>", "nonce": "<hex32>" }`
  - `POST /v1/contract/tx` body: `{ "tx": "<hex32>", "prepared_command": { ... }, "address": "<pubkey-hex32>", "signature": "<hex64>", "nonce": "<hex32>", "sim": true|false }`

Notes:
- To allow wallet tx submission, start the peer with `--rpc` and `--api-tx-exposed` (or env `PEER_API_TX_EXPOSED=1` + `PEER_RPC=1`).
- The peer must be subnet-writable (writer) to broadcast a tx.

## Building your own app (Protocol + Contract)

The runner uses demo protocol/contract files under `src/dev/` (wired in `scripts/run-peer.mjs`) so you can test quickly.

For a real app, you typically:

1) Create a custom Protocol that maps user commands to typed operations:
   - implement `mapTxCommand(command) -> { type, value }`

2) Create a custom Contract that deterministically handles those ops and writes to state:
   - `addFunction("yourOp")` or schemas
   - implement `async yourOp() { await this.put("app/...", ...) }`

All nodes in the subnet must run the same Protocol/Contract logic for deterministic results.

---

## How `/tx` works (the lifecycle)

When you run `/tx --command "..."` in the CLI (or a wallet uses the RPC tx flow), the flow is:

1) The command string is mapped into an operation object: `{ type, value }`.
2) trac-peer hashes and signs the operation and broadcasts a settlement tx to MSB.
3) trac-peer waits until its local MSB view confirms that tx.
4) trac-peer appends a subnet op that references the confirmed MSB tx (so every subnet node can verify it).
5) Every subnet node applies the subnet op and runs contract logic locally, deriving the same results from the same ordered log.

Where does step (1) happen?
- In the demo runner (`scripts/run-peer.mjs`) it’s in the protocol class’s `mapTxCommand(...)` (example: `src/dev/pokemonProtocol.js`).
- The base protocol method is `Protocol.mapTxCommand(...)` in `src/protocol.js`. For your own app you override that function.

Wallet tx flow specifics:
- The wallet sends a typed command (`prepared_command`) and asks the peer to compute `tx` via `POST /v1/contract/tx/prepare`.
- The wallet signs `tx` and submits it to `POST /v1/contract/tx` with `sim: true` to simulate (recommended), then `sim: false` to broadcast.

---

## Reset / clean start

If you want to “start over”, stop the peer and delete its store folder(s):

```sh
rm -rf stores/peer1 stores/peer2 stores/peer3
```

If you only want to change subnet bootstrap generation for one peer, delete just:
- `stores/<peer-store-name>/subnet-bootstrap.hex`

---

## Troubleshooting

### “Requester address not found in state”
MSB doesn’t know your Peer MSB address yet. Fund it on the MSB admin node.

### “ID must be 32-bytes long”
One of your bootstraps is the wrong length. It must be exactly 64 hex characters (32 bytes). Common causes:
- copy/paste includes whitespace
- you accidentally repeated the bootstrap twice in the command
- you passed a non-hex string

### “Subnet deployment broadcast failed”
Usually:
- peer MSB address not funded, or
- MSB validator connectivity issue.

### “TextEncoder is not a constructor” (Pear/Bare)
This happens when a new store needs to generate a new keypair and Pear’s runtime provides a broken `TextEncoder`.
`trac-peer` includes a runtime workaround so you should just re-run after updating; if it persists, confirm you are on the latest workspace state.

### RPC returns 404 on `/health`
Use versioned routes: `/v1/health`.
