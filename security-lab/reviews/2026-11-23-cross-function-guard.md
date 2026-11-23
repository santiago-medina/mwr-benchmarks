# MWR Security Lab — review 2026-11-23

## Baseline and scope
Reviewed `security-lab/audit.py`, `security-lab/test_audit.py`,
`security-lab/js/compare_urls.mjs`, and `.github/workflows/security-lab.yml`.
The last previously verified run available during this review was
[42697340178](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/42697340178),
which completed successfully. Its previous review recorded five Python tests,
a successful isolated ASan/UBSan harness, and 784 URL API comparisons with zero
unresolved differences. These are historical results, **not** verification of
the changes made today.

## New triage finding: cross-function suppression risk
The `annotate_reviewed_isdot` heuristic could still look back 90 lines and
accept an `IsDot` call from a previous function, even if a local
`dotlen == 3` guard appeared in the current function. This could downgrade
an unrelated indexed access. The updated heuristic now refuses to downgrade
when it crosses a top-level closing brace between the helper call and indexed
read. A negative regression test constructs this exact cross-function case.

This is a **scanner false-negative risk**, not a Chromium vulnerability.
The heuristic remains non-semantic; C++ macros, unusual formatting and nested
control flow require manual review. The appropriate long-term replacement is
AST/dataflow analysis or an exact reviewed-site allowlist tied to a source SHA.

## Execution status
The updated files were committed to the public repository, but the latest
GitHub Actions run for these commits could not be retrieved via the available
workflow-run listing interface. Therefore the new regression test and sanitizer
steps are **not claimed to have passed**. No validated Chromium security issue
and no bug bounty submission.
