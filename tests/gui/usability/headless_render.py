#!/usr/bin/env python3
"""Actual CLI rendering corroboration; interaction scenarios stay NOT_TESTED."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time

from usability_corpus import FixtureServer


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--source-repo", type=Path, required=True)
    parser.add_argument("--pillow-prefix", type=Path)
    args = parser.parse_args()
    if args.pillow_prefix:
        sys.path.insert(0, str(args.pillow_prefix / "usr/lib/python3/dist-packages"))
    from PIL import Image
    output, binary = args.output.resolve(), args.binary.resolve()
    output.mkdir(parents=True, exist_ok=False)
    server = FixtureServer(output / "corpus").start()
    rows = []
    try:
        for spec in server.manifest["cases"]:
            row = {**spec, "status": "NOT_TESTED", "scenario_executed": False,
                   "capability": "CLI initial rendering only; no mouse/keyboard/tabs/tick-loop/OS IME",
                   "limitation": "Required interaction is unavailable in this mode"}
            if spec["id"] in ("U10", "U11", "U12"):
                shot = output / (spec["id"] + ".ppm")
                command = [str(binary), "--quiet", "--size", "800x600", "--screenshot", str(shot), server.base + "/" + spec["route"]]
                started = time.monotonic()
                result = subprocess.run(command, capture_output=True, text=True, timeout=15)
                (output / (spec["id"] + ".stdout.txt")).write_text(result.stdout)
                (output / (spec["id"] + ".stderr.txt")).write_text(result.stderr)
                image = Image.open(shot).convert("RGB")
                image.save(output / (spec["id"] + ".png"))
                probes = [(20, 20, [204, 51, 51]), (20, 140, [51, 102, 204])] if spec["id"] == "U10" else [(5, 5, [200, 10, 30])] if spec["id"] == "U12" else []
                checks = [{"page": [x, y], "expected": expected, "actual": list(image.getpixel((x, y)))} for x, y, expected in probes]
                passed = result.returncode == 0 and all(item["expected"] == item["actual"] for item in checks)
                row.update(command=command, exit_code=result.returncode, screenshot_sha256=hashlib.sha256(shot.read_bytes()).hexdigest(),
                           status="PARTIAL" if spec["id"] == "U11" and passed else "PASS" if passed else "FAIL",
                           scenario_executed=True, pixels=checks, elapsed_seconds=round(time.monotonic() - started, 4),
                           limitation="Actual glyph screenshot pending visual comparison" if spec["id"] == "U11" else None)
            rows.append(row)
    finally:
        server.close()
    report = {"schema": "toy-browser-real-usability-v1", "mode": "headless-cli-render-only",
              "source_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=args.source_repo, text=True).strip(),
              "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "fixture_manifest": server.manifest,
              "requests": server.requests(), "cases": rows, "status": dict(Counter(row["status"] for row in rows)),
              "scenarios_executed": sum(row["scenario_executed"] for row in rows)}
    (output / "results.json").write_bytes((json.dumps(report, indent=2, ensure_ascii=False) + "\n").encode())
    print(json.dumps({"status": report["status"], "scenarios_executed": report["scenarios_executed"]}), flush=True)


if __name__ == "__main__":
    main()
