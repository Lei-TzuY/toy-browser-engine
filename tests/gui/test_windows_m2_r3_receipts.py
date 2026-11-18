#!/usr/bin/env python3
"""Regression checks: M2-R3 loopback workbench receipt must not record passwords."""
import sys
from pathlib import Path
import unittest
import urllib.error
import urllib.parse
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parent))
from windows_m2_r3_interactive import WorkbenchServer


class WorkbenchReceiptSafetyTests(unittest.TestCase):
    def setUp(self):
        self.server = WorkbenchServer()
        self.url = self.server.start()

    def tearDown(self):
        self.server.stop()

    def test_password_not_submitted_by_owned_fixture(self):
        with urllib.request.urlopen(self.url, timeout=5) as r:
            html = r.read().decode("utf-8")
        self.assertIn('id="pwd_field" type="password"', html)
        self.assertNotIn('name="pwd_field"', html)

    def test_password_not_persisted_even_when_posted_by_other_client(self):
        data = urllib.parse.urlencode({
            "ime_field": "中文測試",
            "clip_target": "<script>alert(1)</script>",
            "pick": "b",
            "pwd_field": "SENSITIVE_TEST_PASSWORD_DO_NOT_RETAIN",
        }).encode()
        req = urllib.request.Request(self.url + "submit", data=data, method="POST")
        with urllib.request.urlopen(req, timeout=5) as r:
            response = r.read().decode("utf-8")
        self.assertEqual(len(self.server.receipts), 1)
        receipt = self.server.receipts[0]
        self.assertEqual(receipt["fields"]["ime_field"], "中文測試")
        self.assertEqual(receipt["fields"]["pick"], "b")
        self.assertEqual(receipt["redacted_fields"], ["pwd_field"])
        self.assertNotIn("raw_body", receipt)
        self.assertNotIn("pwd_field", receipt["fields"])
        self.assertNotIn("SENSITIVE_TEST_PASSWORD_DO_NOT_RETAIN", str(receipt))
        self.assertNotIn("SENSITIVE_TEST_PASSWORD_DO_NOT_RETAIN", response)
        self.assertIn("&lt;script&gt;", response)
        self.assertNotIn("<script>alert(1)</script>", response)

    def test_unknown_post_and_oversized_body_rejected(self):
        with self.assertRaises(urllib.error.HTTPError) as unknown:
            urllib.request.urlopen(
                urllib.request.Request(self.url + "other", data=b"a=b", method="POST"), timeout=5)
        self.assertEqual(unknown.exception.code, 404)
        unknown.exception.close()
        with self.assertRaises(urllib.error.HTTPError) as large:
            urllib.request.urlopen(
                urllib.request.Request(self.url + "submit", data=b"a" * 16385, method="POST"), timeout=5)
        self.assertEqual(large.exception.code, 413)
        large.exception.close()
        self.assertEqual(self.server.receipts, [])


if __name__ == "__main__":
    unittest.main()
