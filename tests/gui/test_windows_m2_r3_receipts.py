#!/usr/bin/env python3
"""M2-R3 acceptance workbench must not persist secrets or arbitrary user text."""
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

    def submit(self, fields):
        data = urllib.parse.urlencode(fields).encode()
        req = urllib.request.Request(self.url + "submit", data=data, method="POST")
        with urllib.request.urlopen(req, timeout=5) as response:
            return response.read().decode("utf-8")

    def test_password_not_submitted_by_owned_fixture(self):
        with urllib.request.urlopen(self.url, timeout=5) as response:
            markup = response.read().decode("utf-8")
        self.assertIn('id="pwd_field" type="password"', markup)
        self.assertNotIn('name="pwd_field"', markup)

    def test_free_text_and_password_never_enter_receipt(self):
        secret = "SENSITIVE_IME_TEST_PRIVATE_CONTENT"
        password = "SENSITIVE_PASSWORD_DO_NOT_STORE"
        injection = "<script>alert(1)</script>"
        output = self.submit({
            "ime_field": secret,
            "clip_target": injection,
            "pick": "b",
            "pwd_field": password,
            "private_secret_key": "should not persist",
        })
        self.assertEqual(len(self.server.receipts), 1)
        receipt = self.server.receipts[0]
        self.assertEqual(receipt["fields"], {"pick": "b"})
        self.assertEqual(receipt["text_lengths"]["ime_field"], len(secret))
        self.assertEqual(receipt["text_lengths"]["clip_target"], len(injection))
        self.assertEqual(receipt["redacted_fields"], ["clip_target", "ime_field", "pwd_field"])
        self.assertEqual(receipt["unknown_field_count"], 1)
        self.assertFalse(receipt["checks"]["ime_expected_text"])
        self.assertNotIn("raw_body", receipt)
        for forbidden in (secret, password, injection, "private_secret_key", "should not persist"):
            self.assertNotIn(forbidden, str(receipt))
            self.assertNotIn(forbidden, output)
        self.assertIn("&quot;pick&quot;", output)
        self.assertNotIn("<script>", output)

    def test_known_test_strings_are_verifiable_without_storage(self):
        expected_clip = "台灣瀏覽器 Copy/Cut 測試 🌐"
        body = self.submit({
            "ime_field": "中文測試😀",
            "clip_source": expected_clip,
            "clip_target": expected_clip,
            "pick": "b",
            "clicks": "3",
        })
        receipt = self.server.receipts[0]
        self.assertEqual(receipt["fields"], {"pick": "b", "clicks": "3"})
        self.assertEqual(receipt["text_lengths"]["ime_field"], len("中文測試😀"))
        self.assertTrue(all(receipt["checks"].values()))
        self.assertNotIn(expected_clip, body)
        self.assertNotIn("中文測試😀", body)

    def test_untrusted_option_values_never_echo(self):
        response = self.submit({"pick": "<secret>", "clicks": "PASSWORD123"})
        receipt = self.server.receipts[0]
        self.assertEqual(receipt["fields"], {"pick": "INVALID", "clicks": "INVALID"})
        self.assertNotIn("<secret>", response)
        self.assertNotIn("PASSWORD123", response)

    def test_invalid_post_and_large_body_rejected(self):
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
