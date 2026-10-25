#!/usr/bin/env python3
"""Regression checks for evidence accidentally treating skipped tests as passing gates."""
import json
from pathlib import Path
import tempfile
import unittest

from coverage import CHECKLIST, check_mapping, observations


class EvidenceTests(unittest.TestCase):
    def test_inventory_rejects_a_new_unmapped_requirement_or_missing_reference(self):
        data = json.loads(CHECKLIST.read_text())
        data["checks"].pop()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "checklist.yaml"
            path.write_text(json.dumps(data))
            errors, _ = check_mapping(checklist_path=path)
            self.assertTrue(any("PLAN §7" in error for error in errors))
            data = json.loads(CHECKLIST.read_text())
            data["checks"][0]["components"][1]["test"] = "nonexistent_test_cannot_prove_security"
            path.write_text(json.dumps(data))
            errors, _ = check_mapping(checklist_path=path)
            self.assertTrue(any("referenced native test missing" in error for error in errors))

    def test_skipped_and_flaky_browser_results_do_not_satisfy_a_gate(self):
        title = "SEC sandbox cookie writes are inert and Lax sibling canaries receive no cookies"
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "results.json"
            test = {"projectName": "chromium-desktop", "expectedStatus": "passed", "status": "expected", "results": [{"status": "skipped"}]}
            data = {"suites": [{"specs": [{"title": title, "file": "../security/sandbox_cookies.spec.ts", "tests": [test]}]}]}
            path.write_text(json.dumps(data))
            self.assertFalse(observations(playwright_paths=[path]))
            test["results"] = [{"status": "failed"}, {"status": "passed"}]
            path.write_text(json.dumps(data))
            self.assertFalse(observations(playwright_paths=[path]))
            test["results"] = [{"status": "passed"}]
            path.write_text(json.dumps(data))
            observed = observations(playwright_paths=[path])
            self.assertEqual(observed[("playwright", "tests/security/sandbox_cookies.spec.ts", title)], {"chromium-desktop"})

    def test_skipped_junit_is_not_execution_and_qualifier_is_required(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "native.xml"
            path.write_text('<testsuite><testcase classname="pf_caps" name="host_bound"><skipped/></testcase><testcase name="tamper"/></testsuite>')
            self.assertFalse(observations(junit_paths=[path]))
            path.write_text('<testsuite><testcase classname="pf_caps" name="host_bound"/></testsuite>')
            self.assertTrue(observations(junit_paths=[path]))


if __name__ == "__main__":
    unittest.main()
