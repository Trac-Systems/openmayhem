OpenMayhem 0.2.300

- Keep clean capacity and policy refusals out of provider error circuits, so busy providers remain eligible when their slots reopen.
- Defer checkpoint acknowledgement until matching cached, reasoning or tool output has arrived. Receipt signatures, usage and amount verification remain mandatory.
- Preserve bounded price refresh recovery when a current tariff exceeds the previous request quote.
- Recover verified abandoned managed engines before the startup memory check.
- Separate output chunking from receipt cadence: streaming now uses at most 64 billing units per checkpoint, capped by the configured signed policy. Final and cancellation receipts keep exact usage accounting.
- Add optional, bounded verification and receipt-phase timing diagnostics without recording prompts or credentials.

Update buyer gateways together with their matching proxy workers. This release keeps the existing ledger contract and model calibrations.
