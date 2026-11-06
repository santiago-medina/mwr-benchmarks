# MWR Security Lab (experimental)

Automated, **source-only triage** of a narrowly scoped Chromium code surface. This is not a vulnerability scanner with confirmed findings and does not establish Chrome VRP eligibility.

## Current coverage
- Chromium URL path canonicalization: `url/url_canon_path.cc`
- Related reference tests: `url/url_canon_unittest.cc`

The job retrieves only these two public source files (no Chromium checkout or build), records their SHA-256 hashes, emits JSON and Markdown review reports, and uploads the reports as GitHub Actions artifacts.

## Execution
GitHub Actions: [Security Lab workflow](../../actions/workflows/security-lab.yml). It runs on changes and daily, and can be triggered manually.

Locally (Python 3.10+, network required):

```sh
python3 -m unittest discover -s security-lab -p 'test_*.py' -v
python3 security-lab/audit.py
```

## Triage policy
- Every rule output is **needs-human-review**, never a confirmed vulnerability.
- The index-offset rule highlights expressions requiring manual bounds/invariant analysis; it deliberately does **not** imply an out-of-bounds bug.
- Debug-only guards may be safe when enforced by surrounding release-mode checks.
- TODO comments describe known limitations, not flaws by themselves.
- Any suspected Chrome issue must first be corroborated by independent tests and reproduced on an affected official Chrome build with a demonstrable security impact.
- Do not automatically submit reports to bug bounty programs or publish unpatched exploit details.

## Next iterations
Expand narrow source coverage, add actual executable differential and fuzz tests for standalone components where licenses and dependency constraints permit, record exact source revisions, and triage false positives before escalating any issue.
