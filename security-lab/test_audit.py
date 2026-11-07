import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from audit import audit, triage, markdown, annotate_reviewed_isdot

class AuditTests(unittest.TestCase):
    def test_index_expression_is_review_only(self):
        result = triage("output->push_back(path_value[i + 2]);", "url/url_canon_path.cc")
        self.assertEqual([f["rule"] for f in result], ["INDEX_OFFSET"])
        self.assertFalse(result[0]["confirmed_vulnerability"])

    def test_no_false_alarm_for_plain_access(self):
        self.assertEqual(triage("char x = path_value[i];", "url/url_canon_path.cc"), [])

    def test_debug_only_guard_flag(self):
        result = triage("DCHECK(output->length() > 0);", "url/url_canon_path.cc")
        self.assertEqual(result[0]["rule"], "ASSERTION_ONLY")

    def test_guarded_isdot_downgrade_requires_both_contracts(self):
        source = (
            "IsDot(path_value.substr(i))\n"
            "if (dotlen == 3 && is_preserving_percent_encoded_dot_in_path) {\n"
            "  output->push_back(path_value[i + 2]);\n"
            "}"
        )
        helper = (
            "if (spec.size() >= 3 && spec[0] == '%' && spec[1] == '2' && "
            "(spec[2] == 'e' || spec[2] == 'E')) return 3;"
        )
        f = triage(source, "url/url_canon_path.cc")
        self.assertEqual(f[0]["status"], "needs-human-review")
        annotated = annotate_reviewed_isdot(f, {
            "url/url_canon_path.cc": source,
            "url/url_canon_internal.h": helper,
        })
        self.assertEqual(annotated[0]["status"], "reviewed-likely-false-positive")
        self.assertFalse(annotated[0]["confirmed_vulnerability"])
        f2 = triage(source, "url/url_canon_path.cc")
        annotate_reviewed_isdot(f2, {"url/url_canon_path.cc": source})
        self.assertEqual(f2[0]["status"], "needs-human-review")

    def test_report_never_claims_confirmed_issue(self):
        report = audit({"url/url_canon_path.cc": "DCHECK(valid);", "url/url_canon_unittest.cc": "TEST(Example, Ok) {}"})
        self.assertEqual(report["confirmed_vulnerabilities"], 0)
        self.assertEqual(len(report["review_candidates"]), 1)
        self.assertIn("not confirmed vulnerabilities", markdown(report))

if __name__ == "__main__":
    unittest.main()
