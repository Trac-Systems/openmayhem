# OpenMayhem Core 0.2.235

This release validates embedding providers' exact tokenizer usage within the
same conservative bounds used by the signed spend voucher. Valid signed
embedding receipts no longer fail when exact tokenizer counts differ from the
gateway's routing estimate, while incoherent or out-of-bounds usage remains
rejected.

The Intercom contract remains version 25 with unchanged contract bytes. The
signed model catalog is unchanged.
