# MWR Security Lab — 2026-11-16 review

## Verified baseline
GitHub Actions [run 42697340178](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/42697340178), dated 2026-11-09 UTC, completed successfully. Its logs show:
- 5 Python tests passed
- Isolated `IsDot` ASan/UBSan step passed
- 784 URL API comparisons: one known `file:` origin representation difference, zero unresolved
- Six static review locations; no confirmed security vulnerabilities

## Source investigation
Examined Chromium `url/url_canon_path.cc` and `url/url_canon_internal.h`. The two `path_value[i + 2]` accesses are under local `dotlen == 3` guards. The helper's `IsDot` contract only returns 3 when the view contains a full `%2e` or `%2E` escape. The index reads are thus consistent with the helper invariant. This is not an exploitable out-of-bounds report.

## Scanner correction
Previous triage downgraded every exact `path_value[i + 2]` read in the file whenever it found an `IsDot` call and a `dotlen` guard anywhere. This could wrongly suppress an unrelated unguarded read. The updated rule requires a local enclosing guard near each individual read, plus the helper contract and nearby `IsDot` assignment. Added a negative regression test for a second unguarded read.

**Limitation:** this is still a text heuristic, not a C++ control-flow/dataflow proof. The updated workflow has not yet been independently confirmed successful as of this review. No bug bounty submission is warranted.
