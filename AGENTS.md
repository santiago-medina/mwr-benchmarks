# Working agreements

- Commit and push each coherent increment before continuing, especially before a long build or download. The user explicitly requires frequent remote checkpoints so workspace loss cannot erase progress. Label incomplete checkpoints and state which validation is still pending.
- Keep `gc-lab` small and auditable. Reuse ordinary object operations and the runtime's graph mutations instead of introducing parallel property or rooting mechanisms.
- Run relevant contracts and preserve honest evidence. Distinguish Test262 passed, unsupported and failed cases; a baseline regression gate is not full conformance.
- Reuse compatible Servo dependency artifacts. Request the native Actions job with `[servo]` in a push commit or `workflow_dispatch` when validating a native milestone; do not trigger a full Servo build for every incomplete checkpoint.
- Keep the tested commit, source hashes and workflow run identifiable in recorded proof files. Never report a queued or running native job as successful.
