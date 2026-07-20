# Attestation verifier fixtures

- `amd/*` is derived without modification from the Apache-2.0 `sev` 8.0.0
  crate test report/VCEK and built-in Milan ARK/ASK certificates. The PEM
  certificates were mechanically converted to DER.
- `intel/*` is copied from the MIT-licensed `dcap-qvl` 0.5.2 crate sample
  quote, collateral, and Intel trusted root.

These are fixed verification fixtures. Production trust is supplied only by
authenticated admin policy and collateral in verifier stdin.
