# OpenMayhem Core 0.2.234

This release bounds embedding spend reservations by UTF-8 input bytes plus a
fixed per-input special-token allowance. Provider receipts whose exact
tokenizer usage exceeds the routing estimate now remain within the signed
spend ceiling.

The Intercom contract remains version 25 with unchanged contract bytes. The
signed model catalog is unchanged.
