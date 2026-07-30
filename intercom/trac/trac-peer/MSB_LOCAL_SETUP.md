# MSB Local Setup Guide (for `trac-peer` development)

This guide lives in the `trac-peer` repo, but it describes how to run and operate an **MSB (Main Settlement Bus)** node locally so you can fund/whitelist accounts and validate that `trac-peer` transactions settle correctly.

It assumes you have access to an MSB network (bootstrap + channel) and (optionally) an MSB admin keypair/store for whitelisting + initialization operations.

> Scope: This document does **not** modify the MSB repo; it only documents how to use it.

---

## 0) Quick glossary

- **MSB network**: the validator network where transactions “settle”.
- **bootstrap**: a 32-byte hex id (64 hex chars) identifying the MSB Autobase.
- **channel**: discovery topic (string) that MSB nodes must share to find each other.
- **store**: local persisted data for one node (typically under `stores/<storeName>/`).
- **MSB address**: a bech32m address like `trac1...` (account identity).
- **writing key (wk)**: a 32-byte hex key representing the node’s Autobase writer.
- **whitelisted**: account is allowed to apply to become a validator.
- **writer / indexer**: Autobase roles on MSB (validators participate in consensus / indexing).
- **confirmed vs unconfirmed**: confirmed = in MSB signed state, unconfirmed = local unsigned view.

---

## 1) Prerequisites

- Node.js + npm
- Pear (required by MSB repo scripts):

```sh
npm install -g pear
```

Optional (for MSB tests):

```sh
npm install -g bare
```

---

## 2) Install MSB (repo checkout)

From your workspace root (example path):

```sh
git clone -b main --single-branch git@github.com:Trac-Systems/main_settlement_bus.git
cd main_settlement_bus
npm install
```

---

## 3) Run MSB locally

MSB has two common local modes:

### 3.1 Interactive CLI node (recommended for dev)

```sh
MSB_STORE=node1 npm run env-prod
```

This starts an interactive node and stores data under `main_settlement_bus/stores/node1/`.

### 3.2 RPC node (optional, for automation)

```sh
MSB_STORE=rpc-node-store MSB_HOST=127.0.0.1 MSB_PORT=5000 npm run env-prod-rpc
```

This starts MSB plus an HTTP server (the MSB repo’s own RPC).

---

## 4) “Admin mode” and why it matters

MSB has extra admin-only commands (whitelisting, initialization migrations, banning). In the current MSB codebase, **admin mode is inferred by store name**:

- if `MSB_STORE=admin`, MSB will print the admin commands in `/help`.

Start an admin-mode node like:

```sh
MSB_STORE=admin npm run env-prod
```

Important:
- Admin commands require `enableWallet=true` (default in MSB config).
- Some admin actions require that the node is the bootstrap writer (MSB enforces this).

---

## 5) MSB CLI commands you will use (interactive)

Run `/help` inside the MSB console for the full list. The most important ones for local setup are:

### 5.1 Network + state inspection

- `/stats` — prints local key info + DAG lengths.
- `/confirmed_length` — prints confirmed (signed) length.
- `/unconfirmed_length` — prints unconfirmed (unsigned) length.
- `/get_fee` — prints current MSB fee.
- `/get_txv` — prints current tx validity (“txv”) root.

### 5.2 Account inspection

- `/get_balance <address> <confirmed>` — balance query (`confirmed` defaults to `true`).
- `/node_status <address>` — shows node role flags (whitelisted/writer/indexer), license, stake, balance.

### 5.3 Funding / transfers (requires wallet)

- `/transfer <to_address> <amount>` — transfers funds to another address (fee required).

This is the simplest way to “fund” a new account so it exists in MSB state and can pay fees.

### 5.4 Validator role changes (requires whitelisting + stake)

- `/add_writer` — apply to become a validator/writer on MSB.
- `/remove_writer` — remove yourself as a writer (stake refunded per protocol rules).

### 5.5 Subnet deployment entries (for `trac-peer`)

- `/deployment <subnetwork_bootstrap> <channel>` — registers a subnet in MSB (fee required).
- `/get_deployment <subnetwork_bootstrap>` — checks if a subnet bootstrap is registered.

### 5.6 Transaction lookup

- `/get_tx_info <tx_hash>` — decodes and prints the transaction payload (confirmed only).
- `/get_tx_details <tx_hash>` — shows detailed tx info.
- `/get_extended_tx_details <tx_hash> <confirmed>` — detailed view, choose confirmed/unconfirmed.
- `/get_txs_hashes <start> <end>` — list tx hashes by index range.

---

## 6) Whitelisting + initial balances (admin workflow)

MSB supports a “bootstrap initialization phase” (balance migration + whitelisting). These operations read from files in the MSB repo:

- `main_settlement_bus/migration/initial_balances.csv`
- `main_settlement_bus/whitelist/addresses.csv`

### 6.1 Add (or update) initial balances

Edit `migration/initial_balances.csv`:

```csv
trac1...,1000.000000000000000001
trac1...,250.0
```

Then, in an admin-mode MSB console:

```txt
/balance_migration
```

This appends initialization operations and verifies them.

### 6.2 Add whitelist addresses

Edit `whitelist/addresses.csv` (one address per line):

```txt
trac1...
trac1...
```

Then:

```txt
/add_whitelist
```

### 6.3 (Optional) disable initialization

When you’re done bootstrapping:

```txt
/disable_initialization
```

After this, whitelisting and some operations require normal fee/stake rules.

---

## 7) Funding a `trac-peer` node on MSB (required for `/tx`)

When you start a `trac-peer` node, it prints a **Peer MSB address** (`trac1...`).

If MSB doesn’t know that address (no node entry / no balance), `trac-peer` transaction broadcast will fail with errors like:

- “Requester address not found in state”

Fix:
1) On an MSB node that has funds (often your admin wallet), run:

```txt
/transfer <peer_msb_address> <amount>
```

2) Re-try the peer tx.

For local dev, you typically fund each peer with at least a few fees worth of balance.

---

## 8) Connect `trac-peer` to your MSB network

Once MSB is running and you have:
- `msb-bootstrap` (hex32)
- `msb-channel` (string)

follow `trac-peer` instructions in:
- `README.md` (quickstart)
- `DOCS.md` (full guide)

---

## 9) Common troubleshooting

### “Can not initialize an admin - bootstrap is not equal to writing key.”

Admin initialization (`/add_admin`) is only valid on the bootstrap writer. If you are not the bootstrap node (or don’t have the bootstrap writer key), MSB will reject admin creation.

### “Requester address not found in state.”

The account has no node entry yet. Fund it via `/transfer` or include it in initial balance migration.

### “Insufficient balance” / “fee check failed”

Fund the sender, and confirm fee via `/get_fee`.

### “Nothing is confirmed yet”

Confirmed state advances when MSB validators index/confirm. Check:

- `/stats` (signed vs unsigned lengths)
- `/confirmed_length` and `/unconfirmed_length`

