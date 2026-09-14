# 0.2.219

- Admit signed workflow memory from the maximum parts selectable by one request
  while continuing to verify every advertised part in the provider inventory.
- Stream native OpenAI-compatible tool calls that follow assistant commentary,
  preserve the commentary, and fail closed on incomplete tool envelopes.
- Keep ComfyUI request journals removable by the managed sandbox after each
  reference-file workflow.
