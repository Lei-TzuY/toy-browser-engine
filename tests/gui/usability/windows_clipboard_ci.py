#!/usr/bin/env python3
"""Actual Windows CF_UNICODETEXT Copy/Cut/Paste on an isolated GitHub-hosted runner.

Not a Microsoft Bopomofo IME test. Never reads or changes Owner clipboard.
Uses only known synthetic text and trusted local test fixture; fails closed off
the hosted runner, on desktop interference, or without an owned foreground HWND.
"""
from __future__ import annotations
import ctypes as c
from ctypes import wintypes as w
import json
import os
from pathlib import Path
import sys
import time

from native_support import WindowsToy
from usability_corpus import FixtureServer

TEXT = "TBE123中文測試"
CF_UNICODETEXT = 13

def assert_hosted():
    for name, expected in [
        ("GITHUB_ACTIONS", "true"),
        ("RUNNER_ENVIRONMENT", "github-hosted"),
        ("GITHUB_REPOSITORY", "Lei-TzuY/toy-browser-engine"),
    ]:
        if os.environ.get(name) != expected:
            raise RuntimeError("REFUSE_NONEXCLUSIVE_HOST: " + name)

def read_own_test_clipboard() -> bool:
    """Only called after a successful owned-window Ctrl+C action."""
    user = c.WinDLL("user32", use_last_error=True)
    kernel = c.WinDLL("kernel32", use_last_error=True)
    user.OpenClipboard.argtypes = [w.HWND]
    user.OpenClipboard.restype = w.BOOL
    user.GetClipboardData.argtypes = [w.UINT]
    user.GetClipboardData.restype = w.HANDLE
    user.CloseClipboard.argtypes = []
    user.CloseClipboard.restype = w.BOOL
    kernel.GlobalLock.argtypes = [w.HANDLE]
    kernel.GlobalLock.restype = c.c_void_p
    kernel.GlobalUnlock.argtypes = [w.HANDLE]
    kernel.GlobalUnlock.restype = w.BOOL
    for _ in range(25):
        if user.OpenClipboard(None):
            break
        time.sleep(0.04)
    else:
        raise RuntimeError("TEST_CLIPBOARD_UNAVAILABLE")
    try:
        handle = user.GetClipboardData(CF_UNICODETEXT)
        if not handle:
            return False
        addr = kernel.GlobalLock(handle)
        if not addr:
            return False
        try:
            # Unknown clipboard text is never serialized or printed.
            return c.wstring_at(addr) == TEXT
        finally:
            kernel.GlobalUnlock(handle)
    finally:
        user.CloseClipboard()

def expect_title(driver: WindowsToy, expected: str) -> bool:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if driver.title().startswith(expected + " — "):
            return True
        time.sleep(0.06)
    return False

def main():
    assert_hosted()
    base = Path(__file__).resolve().parents[3]
    binary = base / "target/release/browser_engine.exe"
    if not binary.is_file():
        raise RuntimeError("BROWSER_BINARY_MISSING")
    output = base / "ci-windows-native-clipboard"
    output.mkdir(parents=True, exist_ok=False)
    fixture = FixtureServer(output / "corpus").start()
    driver = None
    result = {
        "schema": "m2r4-hosted-windows-real-clipboard-v1",
        "test_text_is_synthetic": True,
        "native_ime_tested": False,
        "owned_foreground_required": True,
        "os_clipboard_copy_match": False,
        "cut_clears_control": False,
        "paste_restores_control": False,
        "status": "FAIL",
    }
    try:
        driver = WindowsToy(binary, fixture.base + "/input.html", output / "owned-gui")
        if not expect_title(driver, "Input"):
            raise AssertionError("INITIAL_INPUT_PAGE_MISSING")
        driver.click(60, 26)
        driver.type_text(TEXT)  # committed Unicode, NOT IME composition
        if not expect_title(driver, "Input " + TEXT):
            raise AssertionError("INITIAL_UNICODE_INPUT_FAILED")
        driver.chord("Control", "a")
        driver.chord("Control", "c")
        result["os_clipboard_copy_match"] = read_own_test_clipboard()
        if not result["os_clipboard_copy_match"]:
            raise AssertionError("OWNED_COPY_NOT_IN_WIN32_CLIPBOARD")
        driver.chord("Control", "x")
        result["cut_clears_control"] = expect_title(driver, "Input")
        if not result["cut_clears_control"]:
            raise AssertionError("CTRL_X_DID_NOT_CLEAR")
        driver.chord("Control", "v")
        result["paste_restores_control"] = expect_title(driver, "Input " + TEXT)
        if not result["paste_restores_control"]:
            raise AssertionError("CTRL_V_DID_NOT_RESTORE")
        result["status"] = "PASS"
    except Exception as e:
        result["error_type"] = type(e).__name__
        result["error_code"] = str(e) if str(e).isupper() else "TEST_ACTION_FAILED"
    finally:
        if driver is not None:
            try:
                driver.close()
            except Exception:
                result["cleanup"] = "WINDOW_CLOSE_FAILED"
        fixture.close()
        (output / "receipt.json").write_text(
            json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(result, ensure_ascii=False), flush=True)
    return 0 if result["status"] == "PASS" else 1

if __name__ == "__main__":
    sys.exit(main())
