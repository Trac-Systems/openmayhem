# OpenMayhem Core 0.2.221

- Let idle workflow engines release retained model and allocator memory when the provider runtime floor activates.
- Use ComfyUI's supported unload and free-memory controls without interrupting active workflow sessions.
- Keep unrelated backends unchanged when they do not support in-place memory reclamation.

This release does not change the Intercom contract.
