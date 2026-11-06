#!/usr/bin/env python3
"""Run the shared controlled corpus on native Windows or isolated Chrome CDP.

Native input is sent only to an owned foreground window. An obscured window
or moving desktop pointer is an environment block, never evidence of a page
regression. Chrome runs headless with a fresh profile; it is a reference
engine observation, not a native GUI or operating-system IME test.
"""
import argparse
import ctypes
import json
import os
from pathlib import Path
import platform
import sys
import time

from native_support import ChromeReference, WindowsToy, digest
from usability_suite import run_suite, save_json


def desktop_preflight():
    from ctypes import wintypes

    samples = []
    for _ in range(12):
        point = wintypes.POINT()
        if not ctypes.windll.user32.GetCursorPos(ctypes.byref(point)):
            raise ctypes.WinError()
        samples.append([point.x, point.y])
        time.sleep(0.1)
    return {
        "cursor_samples": samples,
        "distinct_positions": len(set(map(tuple, samples))),
        "modifiers_down": [
            key for key in (16, 17, 18, 91, 92)
            if ctypes.windll.user32.GetAsyncKeyState(key) & 0x8000
        ],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mode", choices=("native", "chrome"), required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--revision", help="Source annotation; optional build receipt verifies the binary")
    parser.add_argument("--build-receipt", type=Path)
    parser.add_argument("--selected", help="Comma-separated scenario IDs")
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.error("Run this adapter with native Windows Python; WSL is a separate platform")
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.exists():
        parser.error("Output already exists; preserve prior attempts and choose a new directory")
    provenance = {
        "mode": args.mode, "run_device": platform.node(),
        "run_os": platform.platform(), "source_revision_annotation": args.revision,
        "binary": str(binary), "binary_sha256": digest(binary),
        "native_driver_sha256": digest(Path(__file__).with_name("native_support.py")),
    }
    if args.build_receipt:
        receipt = json.loads(args.build_receipt.read_text(encoding="utf-8"))
        if receipt["binary_sha256"] != provenance["binary_sha256"]:
            parser.error("Build receipt does not match this binary")
        provenance["verified_build_receipt"] = receipt
    lock = None
    if args.mode == "native":
        preflight = desktop_preflight()
        save_json(output.parent / (output.name + "-desktop-preflight.json"), preflight)
        if preflight["distinct_positions"] != 1 or preflight["modifiers_down"]:
            parser.error("Desktop input interference; no native scenario launched")
        lock = output.parent / "native-usability-gui.lock"
        descriptor = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
        with os.fdopen(descriptor, "w") as handle:
            handle.write(str(os.getpid()))
        factory = lambda url, out: WindowsToy(binary, url, out)
    else:
        factory = lambda url, out: ChromeReference(binary, url, out)
    try:
        report = run_suite(factory, output, provenance,
                           args.selected.split(",") if args.selected else None)
        print(json.dumps({"results": str(output / "results.json"),
                          "status": report["status"], "executed": report["executed"]}), flush=True)
        return 1 if report["status"].get("FAIL", 0) else 0
    finally:
        if lock:
            lock.unlink()


if __name__ == "__main__":
    raise SystemExit(main())
