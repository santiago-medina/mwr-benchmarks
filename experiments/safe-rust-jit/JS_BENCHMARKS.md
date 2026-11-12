# JavaScript benchmark pilot

This is a small original category-based pilot informed by [JetStream 2.2](https://browserbench.org/JetStream2.2/) and SunSpider/Octane. It does not copy or execute official suite cases and the numbers are NOT JetStream scores.

Six JavaScript workloads: integer bit operations, arrays, object properties, strings, JSON and regular expressions. Node.js/V8 runs deterministic inputs, warmup, seven rounds, median time per operation, and checksums.

IMPORTANT: The experimental KVM guest currently runs a fixed x86 arithmetic kernel, NOT JavaScript. The JS results are therefore a Node/V8 baseline, not native-vs-KVM JavaScript results. The separate native-vs-KVM microbenchmark remains in comparison_results.csv.

Next milestone: implement a minimal JS frontend with matching semantics in the host and guest, then run equivalent JS programs on both. No claims of JS support in KVM until implemented.

Run locally: cd experiments/safe-rust-jit && node js_subset.js
