import pathlib
import sys
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from audit import audit, triage, markdown

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

    def test_report_never_claims_confirmed_issue(self):
        report = audit({"url/url_canon_path.cc": "DCHECK(valid);", "url/url_canon_unittest.cc": "TEST(Example, Ok) {}"})
        self.assertEqual(report["confirmed_vulnerabilities"], 0)
        self.assertEqual(len(report["review_candidates"]), 1)
        self.assertIn("not confirmed vulnerabilities", markdown(report))

if __name__ == "__main__":
    unittest.main()
