#!/usr/bin/env python3
"""Read-only Win32 HKL/IMM inventory on a throwaway GitHub-hosted runner.

Does NOT enable, install, activate or test actual Chinese input methods.
A listed IME is evidence of availability only; not composition functionality.
"""
from __future__ import annotations
import ctypes as c
from ctypes import wintypes as w
import json
import os
from pathlib import Path
import sys

def main() -> int:
    if not (
        os.environ.get("GITHUB_ACTIONS") == "true"
        and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted"
        and os.environ.get("GITHUB_REPOSITORY") == "Lei-TzuY/toy-browser-engine"
    ):
        print("REFUSE_NON_HOSTED_IME_INVENTORY", file=sys.stderr)
        return 2

    user = c.WinDLL("user32", use_last_error=True)
    imm = c.WinDLL("imm32", use_last_error=True)
    user.GetKeyboardLayoutList.argtypes = [c.c_int, c.POINTER(w.HANDLE)]
    user.GetKeyboardLayoutList.restype = c.c_int
    user.GetKeyboardLayout.argtypes = [w.DWORD]
    user.GetKeyboardLayout.restype = w.HANDLE
    imm.ImmIsIME.argtypes = [w.HANDLE]
    imm.ImmIsIME.restype = w.BOOL
    imm.ImmGetDescriptionW.argtypes = [w.HANDLE, w.LPWSTR, w.UINT]
    imm.ImmGetDescriptionW.restype = w.UINT
    n = user.GetKeyboardLayoutList(0, None)
    buffer = (w.HANDLE * n)() if n else None
    count = user.GetKeyboardLayoutList(n, buffer) if n else 0
    loaded = []
    for hkl in list(buffer or [])[:count]:
        description = c.create_unicode_buffer(256)
        imm.ImmGetDescriptionW(hkl, description, len(description))
        loaded.append({
            "hkl": hex(hkl & 0xffffffffffffffff),
            "language_id": f"{hkl & 0xffff:04x}",
            "is_imm_ime": bool(imm.ImmIsIME(hkl)),
            "imm_description": description.value,
        })
    active = user.GetKeyboardLayout(0)
    report = {
        "scope": "loaded HKLs of this GitHub-hosted worker thread only",
        "not_a_real_ime_test": True,
        "windows_bopomofo_composition": "NOT_TESTED",
        "active_hkl": hex(active & 0xffffffffffffffff) if active else None,
        "loaded_layouts": loaded,
        "zh_tw_loaded": any(x["language_id"] == "0404" for x in loaded),
        "bopomofo_name_present": any("bopomofo" in x["imm_description"].lower() for x in loaded),
        "limitations": ["Installed-but-unloaded TSF IMEs may not appear in GetKeyboardLayoutList",
                        "No TSF candidate UI, preedit, cancellation or commit verified"],
    }
    dest = Path("ci-windows-ime-probe")
    dest.mkdir(exist_ok=False)
    (dest / "inventory.json").write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False))
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
