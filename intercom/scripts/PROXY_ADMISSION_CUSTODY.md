# Admission receiving-address custody

This operator tool exports a signed public inventory for the separate $10 proxy
admission fee. It does not generate keys, transfer funds, sweep balances, verify a
deposit, issue a permit or register a provider. It has no network client. Retail
credit destinations and provider payout identities are not changed.

Run `node scripts/proxy-admission-custody.mjs export /absolute/private/config.json`
on the Unix custody host, from the installed Intercom directory. Redirect stdout
to a public inventory file. Copy only that inventory to the SITE operator host.
Existing receiving keys and passwords remain on the custody host. The tool uses
existing encrypted Trac wallet files or encrypted PKCS8 PEM keys (Ed25519 for TNK,
secp256k1 for TAP). A different key format must be securely converted by its
custodian first; do not put an unencrypted key into this configuration.

```json
{
  "schema_version": 1,
  "purpose": "proxy_admission_receiver",
  "receivers": [{
    "collection": {
      "rail": "TAP",
      "chain_id": 1,
      "token_contract": "<lowercase token contract address>",
      "destination": "<lowercase receiving address>"
    },
    "custody_reference": "operator-inventory-001",
    "format": "encrypted_pkcs8",
    "key_file": "/absolute/private/receiving-key.pem",
    "password_file": "/absolute/private/receiving-key.password"
  }]
}
```

For TNK, `collection` is instead
`{"rail":"TNK","network":"mainnet","destination":"<trac1 address>"}`;
`testnet1` requires a `testtrac1` address. Use `format: "trac_wallet"` for the
existing encrypted Trac JSON format. Password files contain the exact password
bytes: a trailing newline is part of the password. Configuration, passwords and
key files must be owner-only, canonical absolute paths to bounded regular files;
symlinks and hard links are rejected. Windows ACL validation is not implemented,
so this operator action refuses Windows rather than claiming equivalent checks.

Each proof binds the rail, network/chain, token, receiving address, opaque custody
reference and public key under its own signature domain. It verifies possession
at export time. It does **not** establish organizational ownership, backup
availability, solvency or future custody. Verify backups/recovery separately
before activating collection. Custody references identify private operator
inventory; do not use secret filenames, passwords or private keys as references.

`node scripts/proxy-admission-custody.mjs verify /absolute/public/inventory.json`
checks all proofs and emits only verified public inventory. It rejects repeated
physical receiving addresses, even with different object ordering or TAP tokens.
Each batch holds 1–128 receivers and at most 256 KiB. Additional batches have no
total inventory cap. Exports with any invalid entry produce no partial output.

The SITE importer defaults to dry-run and accepts only public inventory. It uses
the operator-selected installed Core verifier, with a sanitized child environment,
bounded output and timeout; the verifier and its dependency tree are trusted
operator-installed code, never a provider-supplied executable or browser path.
See the SITE `docs/proxy-admission-collection.md` for the database import command.
Activation, real deposits and sweeping remain subject to the release gate.
