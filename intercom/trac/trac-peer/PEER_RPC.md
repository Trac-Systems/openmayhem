# trac-peer RPC (HTTP) — API Reference

This is a **reference** for the public HTTP RPC exposed by `trac-peer`.

Base URL example:
- `http://127.0.0.1:5001`

All endpoints below are under the `/v1` prefix.

## Conventions

- All responses are JSON.
- Request bodies (where applicable) are JSON.
- Hex formats:
  - `hex32`: 32-byte hex string (64 hex chars)
  - `hex64`: 64-byte hex string (128 hex chars)

## Errors

Error responses have the shape:

```json
{ "error": "message" }
```

Common status codes:
- `200` success
- `400` bad request (missing/invalid parameters)
- `404` not found (unknown route)
- `413` request body too large
- `500` internal error

---

## `GET /v1/health`

Health check.

### Response `200`

```json
{ "ok": true }
```

---

## `GET /v1/status`

Returns a status summary for the running peer and its MSB client view.

### Query parameters
None

### Response `200`

Object with:
- `peer`: identifiers + subnet view info (writability, signed length, bootstrap, etc.)
- `msb`: MSB bootstrap/networkId/signedLength as seen by this peer’s MSB client

---

## `GET /v1/contract/schema`

Returns an ABI-like schema describing:
- which contract tx types exist (`contract.txTypes`)
- optional per-tx input structure (`contract.ops`)
- the Protocol API method schema (`api.methods`)

### Query parameters
None

### Response `200`

```json
{
  "schemaVersion": 1,
  "schemaFormat": "json-schema",
  "contract": {
    "contractClass": "TuxemonContract",
    "protocolClass": "TuxemonProtocol",
    "txTypes": ["catch"],
    "ops": {
      "catch": { "value": {} }
    }
  },
  "api": { "methods": {} }
}
```

---

## `GET /v1/contract/nonce`

Generates a nonce for signing.

### Query parameters
None

### Response `200`

```json
{ "nonce": "<hex32>" }
```

---

## `GET /v1/contract/tx/context`

Returns the MSB transaction context needed by a client/dapp to compute the `tx` hash locally.

### Query parameters
None

### Response `200`

```json
{
  "msb": {
    "networkId": 918,
    "txv": "<hex32>",
    "iw": "<hex32>",
    "bs": "<hex32>",
    "mbs": "<hex32>",
    "operationType": 12
  }
}
```

---

## `POST /v1/contract/tx`

Simulates or broadcasts a signed contract transaction.

### Request body (JSON)

Required fields:
- `tx` (`hex32`): transaction hash computed by the client/dapp
- `prepared_command` (`object`): `{ "type": "<string>", "value": <any> }`
- `address` (`hex32`): wallet public key (hex) used for signature verification
- `signature` (`hex64`): ed25519 signature over `tx` bytes
- `nonce` (`hex32`)

Optional:
- `sim` (`boolean`, default `false`): when `true`, run MSB preflight + contract simulation; when `false`, broadcast

Example:

```json
{
  "tx": "<hex32>",
  "prepared_command": { "type": "catch", "value": {} },
  "address": "<hex32>",
  "signature": "<hex64>",
  "nonce": "<hex32>",
  "sim": true
}
```

### Response `200`

```json
{ "result": {} }
```

Result shape is protocol-dependent.

---

## `GET /v1/state`

Reads a single key from the subnet state (Hyperbee).

### Query parameters

- `key` (required, string): the exact Hyperbee key to read
- `confirmed` (optional, boolean, default `true`):
  - `true`: read from signed/confirmed view
  - `false`: read from latest local view

### Response `200`

```json
{
  "key": "app/tuxedex/<pubKeyHex>",
  "confirmed": false,
  "value": {}
}
```
