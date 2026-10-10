# Proxy buyer peer configuration

A proxy buyer's financial RPC peer must authenticate as the same wallet as
`buyer_pubkey` and the gateway's unlocked buyer wallet. A healthy discovery peer
with a different transport identity is not a financial substitute: its quote
belongs to another account, even when it follows the same canonical ledger.

By default, `peer_rpc_url` serves both purposes. It must match the gateway's
`--rpc-url`, and the configured network must match the gateway's canonical pins.
Existing configurations retain this behavior.

An operator can explicitly set `financial_rpc_url` in the protected proxy buyer
configuration when a separate wallet-owning peer provides financial services:

```json
{
  "peer_rpc_url": "http://127.0.0.1:19001/v1",
  "financial_rpc_url": "http://127.0.0.1:19002/v1"
}
```

These are fields within the existing configuration, not a complete configuration.
The override applies only to proxy financial observations and publication. It
does not move native inference, discovery, the gateway's ledger watcher, its
wallet, or its retained journals. The proxy bridge must also authenticate the
correct buyer identity.

The financial client continues to require HTTPS or literal loopback HTTP, rejects
URL credentials/query strings/fragments, disables redirects, and bounds reads.
Every canonical observation must match the configured network, contract, wallet,
fresh challenge, and exact purchase. A different URL cannot relax these checks or
authorize spending. Public requests cannot supply this setting.

Before activation, verify the selected peer's canonical prefix and wallet identity,
then inspect a fresh quote for each enabled payment rail. Do not reset, clone or
replace existing ledger or purchase stores to repair an identity mismatch. Keep
native gateway reads on their established peer and reload only the configured
buyer gateways. Validate a limited paid request and its settlement before widening
admission.
