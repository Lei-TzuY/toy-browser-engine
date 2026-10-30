#!/usr/bin/env python3
"""Headless executable smoke; trusted built-in demo and loopback HTTP only."""
import argparse
import hashlib
import http.server
import json
from pathlib import Path
import subprocess
import threading


def smoke(binary, output):
    binary, output = Path(binary).resolve(), Path(output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    checks = []

    def run(name, arguments, expected=0):
        result = subprocess.run([str(binary), *arguments], cwd=output,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=30)
        (output / (name + ".log")).write_bytes(result.stdout)
        if result.returncode != expected:
            raise AssertionError((name, result.returncode, result.stdout.decode("utf-8", "replace")))
        checks.append({"case": name, "exit_code": result.returncode, "passed": True})
        return result

    result = run("help", ["--help"])
    assert b"Usage: browser_engine" in result.stdout
    image = output / "demo.ppm"
    run("built-in-demo", ["--quiet", "--screenshot", str(image)])
    raw = image.read_bytes()
    magic, size, maximum, pixels = raw.split(b"\n", 3)
    width, height = map(int, size.split())
    assert magic == b"P6" and maximum == b"255"
    assert (width, height) == (800, 600) and len(pixels) == width * height * 3
    checks.append({"case": "demo-pixels", "passed": True, "width": width, "height": height,
                   "sha256": hashlib.sha256(raw).hexdigest()})

    class Missing(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()

        def log_message(self, *unused):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Missing)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        result = run("http-404", ["--quiet", "http://127.0.0.1:" + str(server.server_port) + "/missing"], 1)
        assert b"404" in result.stdout
    finally:
        server.shutdown()
        server.server_close()
    receipt = {"binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
               "checks": checks, "passed": len(checks), "network": "loopback only"}
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    return receipt


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    print(json.dumps(smoke(args.binary, args.output), indent=2))
