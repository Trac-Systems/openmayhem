# OpenMayhem Core 0.2.254

Market prices now follow absolute settled utilization instead of comparing activity with the previous epoch. A market at or above 80% utilization increases every price term by 10%, a market at or below 20% decreases every term by 10%, and the middle band holds. The existing 25%-400% reference-price bounds remain.

Contract version 27 and receipt schema 12 bind provider compute time and execution-slot capacity to signed receipts. The same controller applies to text, embeddings, image, video, audio and workflow markets without model recalibration.

Retained receipts from contract versions 23 through 26 continue to settle without signature rewriting. Because those receipts predate signed slot-time evidence, their market holds price for that settlement epoch and resumes utilization pricing with subsequent schema-12-only evidence.

The release also exposes utilization derivations in price reports and retires the former EMA, gain and configurable-step parameters from the writable admin surface.
