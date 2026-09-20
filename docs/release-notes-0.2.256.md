# OpenMayhem Core 0.2.256

- Classify invalid provider requests and invalid model output without penalizing healthy routes.
- Treat signed provider failure receipts as terminal so paid attempts are never dispatched twice.
- Apply the corrected failure handling consistently across text, embedding, image, audio, video, and workflow requests.

This is a wire-compatible code release. Contract version 27 and its contract digest are unchanged.
