#!/usr/bin/env python3
"""Execute actual X11 scenarios on a release binary in an isolated display."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--source-repo", type=Path, required=True)
    parser.add_argument("--tool-prefix", type=Path)
    parser.add_argument("--font", type=Path)
    parser.add_argument("--fallback-font", type=Path)
    parser.add_argument("--cases", nargs="+")
    args = parser.parse_args()
    if args.tool_prefix:
        sys.path.insert(0, str(args.tool_prefix / "usr/lib/python3/dist-packages"))
    from usability_suite import run_suite
    from x11_support import X11Session, X11Toy
    binary = args.binary.resolve()
    provenance = {"source_head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=args.source_repo, text=True).strip(),
                  "source_status": subprocess.check_output(["git", "status", "--short"], cwd=args.source_repo, text=True),
                  "binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                  "platform": "WSL Linux real X11 window on owned Xvfb display", "font_override": str(args.font) if args.font else None}
    session = X11Session(args.output / "display", args.tool_prefix, args.font, args.fallback_font)
    provenance["effective_font_paths"] = {key: session.env.get(key) for key in
                                         ("BROWSER_ENGINE_FONT", "BROWSER_ENGINE_FALLBACK_FONT")}
    provenance["configured_font_sha256"] = {key: hashlib.sha256(Path(value).read_bytes()).hexdigest()
                                            for key, value in provenance["effective_font_paths"].items()
                                            if value and Path(value).is_file()}
    try:
        report = run_suite(lambda url, out: X11Toy(session, binary, url, out), args.output, provenance, args.cases)
    finally:
        session.close()
    print(json.dumps({"source_head": provenance["source_head"], "status": report["status"], "executed": report["executed"]}), flush=True)
    return 1 if report["status"].get("FAIL") else 0


if __name__ == "__main__":
    raise SystemExit(main())
