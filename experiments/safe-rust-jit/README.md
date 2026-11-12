# Safe Rust execution-boundary experiment

This is **not yet a JIT or KVM benchmark**. It measures a 64-round arithmetic kernel called directly in-process versus through a persistent child process using pipes. Each request executes 1, 16 or 256 kernels. Both implementations use safe Rust only (`#![forbid(unsafe_code)]`).

Run `cargo test` and `cargo run --release` from this directory. Results are median nanoseconds per request over 9 measured repetitions, plus amortized nanoseconds per kernel. Results are machine-dependent; native and process timings include their respective loop overheads.

The workflow probes /dev/kvm but does not claim to execute a VM. A future experiment must add an actual KVM backend, and GitHub-hosted runner support for nested virtualization must be verified.
