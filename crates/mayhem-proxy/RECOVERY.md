# Local proxy buyer recovery

This is an explicit control command for an existing buyer recovery database. It
does not enable proxy serving, negotiate new purchases, dispatch inference, or
change native provider and payout services. Automatic supervision and the
authenticated buyer/provider exchange are separate integration work.

```sh
mayhem proxy buyer-recovery once --config buyer-recovery.json --locked
mayhem proxy buyer-recovery watch --config buyer-recovery.json \
  --keypair /path/to/existing/keypair.json \
  --wallet-password-file /path/to/private/password-file
```

`once` processes one bounded pending page. A reported `more: true` means further
keys remain after that page; a failed observation makes the command exit nonzero
without discarding the saved record. `watch` resumes automatically, paces work,
backs off shared RPC failures, and handles SIGINT/SIGTERM. It prints one final
summary on exit rather than logging every poll or retaining a growing event log.
Supervisors can consume the library's coalesced health watch channel.

Configuration has the following shape. Replace identity placeholders with the
buyer's actual network configuration and public key; do not put secrets here.

```json
{
  "schema_version": 1,
  "network": {
    "network_id": "NETWORK_ID",
    "msb_bootstrap": "64_LOWERCASE_HEX_CHARACTERS",
    "subnet_bootstrap": "64_LOWERCASE_HEX_CHARACTERS",
    "contract_version": 30
  },
  "buyer_pubkey": "64_LOWERCASE_HEX_CHARACTERS",
  "peer_rpc_url": "http://127.0.0.1:PORT/v1",
  "recovery_file": "buyer-recovery.redb",
  "max_records": 10000,
  "closed_retention_ms": 604800000,
  "recovery": {
    "page_size": 16,
    "schedule": {
      "interval_ms": 30000,
      "page_pause_ms": 10,
      "retry_initial_ms": 1000,
      "retry_max_ms": 30000,
      "jitter_percent": 20
    }
  }
}
```

Use the actual peer's contract version. The persistent wallet/network identity
does not pin a release; previously accepted original terms are retained. Relative
database paths resolve beside the configuration file. The database requires a
private regular file in an owner-private directory and an exclusive owner. Only
one recovery process may open it. The local file is bounded to 16 KiB; pending and
prune pages are bounded to 64 records, independently of the total record quota.
The retention example governs *resolved local records*, never unknown jobs or
on-chain financial obligations. Records retain their original prune deadline.

Without `--locked`, startup unlocks the existing Core wallet and verifies its
public key before opening the database. No signing key is sent to RPC or a model.
Locked operation can observe canonical state and replay previously signed saved
intentions. It reports `awaiting_wallet` when a permitted action needs a new
signature. An approved outcome must already have passed independent buyer
verification; this worker never approves an unseen result.

Expiry requires the original buyer-approved expiry policy and a fresh canonical
epoch beyond its deadline and grace. It releases only the applicable financial
hold; unknown inference remains unknown and cannot be retried or treated as spare
provider capacity. Transport acknowledgment alone does not prove confirmation.
Interrupted publication resumes with the same saved identity and envelope.

These scheduling values are local recovery defaults, not inference timeouts,
provider concurrency limits, or public rollout acceptance.
