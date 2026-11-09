# Security Lab review — 2026-11-09

## Verified GitHub Actions run

[Run 42620838295](https://github.com/santiago-medina/mwr-benchmarks/actions/runs/42620838295) completed successfully on 2026-11-09 (UTC).

- Python unit tests: **5/5 passed**.
- Isolated `IsDot` contract harness with AddressSanitizer and UndefinedBehaviorSanitizer: **passed**. This is a copied narrow helper, not a Chromium build.
- Node.js v24.22.2 versus Google Chrome 156.0.8078.79 WHATWG URL comparison: **784 cases, 1 difference**.
- Static triage: **6 indicators**, including two previously reviewed likely false positives. No confirmed vulnerabilities.

## Differential result

Input: `file:///a/../b`

Both implementations produce the same `href` (`file:///b`) and pathname (`/b`). Only `origin` differs:
- Node: `null`
- Chrome: `file://`

This is a **file URL origin representation compatibility difference**, not proof of a security bug or an origin-policy bypass. No navigation, cross-origin access, or Chrome security boundary was tested. The test runner now records this narrowly matched case as a known compatibility difference and leaves all other mismatches open for review.

## Additional source review

`url/url_canon_internal.h` lines 277–278 contain `DCHECK`s for valid Unicode scalar ranges in `DoAppendUtf8`. The surrounding documentation states the input must already be validated. Caller analysis would be needed to establish whether untrusted invalid values can reach this function; the assertions alone are not findings.

## Limits and next steps

- The differential suite checks JavaScript's public `URL` API; it does not compile Chromium's URL library.
- The fixed seed creates a reproducible corpus, not exhaustive fuzzing.
- The current implementation of the scanner uses string-pattern heuristics and does not prove call-site invariants.
- Confirm any suspected security impact on official Chrome before any VRP submission.
- Re-run the updated workflow and verify the known/unresolved counters; do not assume the update has passed until GitHub Actions confirms it.
