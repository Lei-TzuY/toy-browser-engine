#!/usr/bin/env python3
"""Run shared real-browser scenarios through an owned GUI/reference adapter.

No browser or server starts on import. Call run_suite only after the execution
gate. The adapter performs actual input; this checker reads captions, requests
and owned screenshots, never toy product internals or script-source substrings.
"""
from collections import Counter
import hashlib
import json
from pathlib import Path
import re
import time
import traceback
import urllib.parse

from usability_corpus import FixtureServer, PAGES, UnavailableEndpoint


def save_json(path, value):
    Path(path).write_bytes((json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode("utf-8"))


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


class CapabilityUnavailable(RuntimeError):
    pass


class Case:
    def __init__(self, spec, server, factory, output):
        self.spec, self.server, self.factory, self.output = spec, server, factory, output
        self.adapter, self.actions, self.observations, self.checks = None, [], [], []
        self.limitations, self.started, self.request_mark = [], time.monotonic(), len(server.requests())

    def invoke(self, method, *args):
        row = {"method": method, "arguments": [str(value) if isinstance(value, Path) else value for value in args], "at_seconds": round(time.monotonic() - self.started, 6)}
        self.actions.append(row)
        try:
            result = getattr(self.adapter, method)(*args)
            row["outcome"] = "returned"
            return result
        except Exception as error:
            row.update(outcome="exception", error=repr(error))
            raise

    def title(self):
        return self.adapter.title()

    def url(self):
        if hasattr(self.adapter, "url"):
            return self.adapter.url()
        caption = self.title()
        return caption.rsplit(" — ", 1)[-1].rsplit(" [", 1)[0] if " — " in caption else None

    def observe(self, label):
        row = {"label": label, "at_seconds": round(time.monotonic() - self.started, 6), "title": self.title(), "url": self.url()}
        self.observations.append(row)
        return row

    def check(self, name, passed, expected, actual):
        self.checks.append({"name": name, "passed": bool(passed), "expected": expected, "actual": actual})
        return bool(passed)

    def expect_title(self, expected, timeout=6):
        end, actual = time.monotonic() + timeout, ""
        while time.monotonic() < end:
            actual = self.title()
            if actual == expected or actual.startswith(expected + " — "):
                break
            time.sleep(0.05)
        self.observe("title expectation: " + expected)
        return self.check("live title", actual == expected or actual.startswith(expected + " — "), expected, actual)

    def expect_url(self, suffix):
        actual = self.url()
        return self.check("committed URL", actual is not None and actual.endswith(suffix), "ends with " + suffix, actual)

    def shot(self, label, probes=()):
        from PIL import Image
        path = self.output / (label + ".png")
        self.invoke("screenshot", path)
        image = Image.open(path).convert("RGB")
        origin = tuple(getattr(self.adapter, "page_origin", (0, 68)))
        row = {"label": label, "path": str(path), "sha256": digest(path), "size": list(image.size), "page_origin": list(origin), "pixels": []}
        for x, y, expected in probes:
            point = (x + origin[0], y + origin[1])
            actual = list(image.getpixel(point))
            row["pixels"].append({"page": [x, y], "expected": list(expected), "actual": actual})
            self.check("visible page pixel", actual == list(expected), {"at": [x, y], "rgb": list(expected)}, actual)
        self.observations.append(row)
        return row

    def requests(self):
        return self.server.requests()[self.request_mark:]

    def expect_request(self, path, method="GET", values=None):
        candidates = []
        end = time.monotonic() + 2
        while time.monotonic() < end:
            candidates = [row for row in self.requests() if row["path"] == path and row["method"] == method]
            if candidates:
                break
            time.sleep(0.04)
        self.check("server request", bool(candidates), {"path": path, "method": method}, candidates)
        if values is not None:
            actual = [urllib.parse.parse_qs(row["body"] if method == "POST" else row["query"], keep_blank_values=True) for row in candidates]
            expected = {key: [value] for key, value in values.items()}
            self.check("server captured form values", any(all(item.get(key) == value for key, value in expected.items()) for item in actual), expected, actual)
            if method == "POST":
                self.check("POST content type", any((row["content_type"] or "").lower().startswith("application/x-www-form-urlencoded") for row in candidates), "application/x-www-form-urlencoded", [row["content_type"] for row in candidates])

    def expect_tabs(self, number):
        actual = self.adapter.tab_count() if hasattr(self.adapter, "tab_count") else None
        if actual is None:
            self.limitations.append("Actual tab count is unavailable in this adapter")
        else:
            self.check("observed tab count", actual == number, number, actual)

    def click(self, x=60, y=26):
        self.invoke("click", x, y)

    def navigate(self, route):
        self.invoke("navigate", self.server.base + route)

    def execute(self):
        cid = self.spec["id"]
        if cid == "U24":
            return "BLOCKED", "No adapter implements actual OS IME composition; committed Unicode is distinct"
        route = self.spec["route"]
        url = (self.server.site / route).as_uri() if cid == "U01" else self.server.base + "/" + route.lstrip("/")
        self.adapter = self.factory(url, self.output)
        expected_start = re.search(r"<title>(.*?)</title>", PAGES.get(route, ""))
        if expected_start:
            self.expect_title(expected_start.group(1))
        else:
            end = time.monotonic() + 6
            while time.monotonic() < end and not self.title().startswith("Reload "):
                time.sleep(0.05)
            self.check("initial reload title", self.title().startswith("Reload "), "Reload N", self.title())
        self.shot("initial")

        if cid in ("U01", "U03", "U04", "U05"):
            self.click()
            self.expect_title("FileTarget" if cid == "U01" else "Target")
            self.expect_url("/file-target.html" if cid == "U01" else "/target.html")
            if cid != "U01":
                self.expect_request("/target.html")
            if cid in ("U04", "U05"):
                self.invoke("back")
                self.expect_title("Home")
                self.expect_url("/index.html")
            if cid == "U05":
                self.invoke("forward")
                self.expect_title("Target")
                self.expect_url("/target.html")
        elif cid == "U02":
            self.navigate("/target.html")
            self.expect_title("Target")
            self.expect_url("/target.html")
            self.expect_request("/target.html")
        elif cid == "U06":
            before = len([row for row in self.requests() if row["path"] == "/reload"])
            self.invoke("reload")
            end = time.monotonic() + 6
            while time.monotonic() < end and len([row for row in self.requests() if row["path"] == "/reload"]) <= before:
                time.sleep(0.05)
            actual = len([row for row in self.requests() if row["path"] == "/reload"])
            self.check("reload server hit increment", actual == before + 1, before + 1, actual)
            self.expect_title("Reload " + str(actual))
        elif cid == "U07":
            self.invoke("new_tab")
            time.sleep(0.3)
            self.observe("new tab initial destination")
            self.expect_tabs(2)
            self.navigate("/target.html")
            self.expect_title("Target")
            self.expect_request("/target.html")
        elif cid == "U08":
            self.click()
            self.invoke("type_text", "first")
            self.expect_title("TabState first")
            self.invoke("new_tab")
            self.navigate("/tabstate.html")
            self.expect_title("TabState")
            self.click()
            self.invoke("type_text", "second")
            self.expect_title("TabState second")
            self.invoke("select_tab", 0)
            self.expect_title("TabState first")
            self.invoke("select_tab", 1)
            self.expect_title("TabState second")
            self.invoke("select_tab", 0)
            self.click(60, 90)
            self.expect_title("EchoGET")
            self.expect_request("/echo-get", values={"q": "first", "case": "U08"})
        elif cid == "U09":
            self.invoke("new_tab")
            self.navigate("/target.html")
            self.expect_title("Target")
            self.invoke("close_tab")
            self.expect_title("Home")
            self.expect_tabs(1)
        elif cid == "U10":
            self.shot("solid-colors", [(20, 20, [204, 51, 51]), (20, 140, [51, 102, 204])])
        elif cid == "U11":
            self.limitations.append("Actual Unicode screenshot captured; pending visible glyph review against actual reference. DOM/title is not glyph evidence")
        elif cid == "U12":
            self.shot("png-image", [(5, 5, [200, 10, 30])])
            self.expect_request("/tile.png")
        elif cid == "U13":
            self.click()
            self.expect_title("Click 1")
        elif cid == "U14":
            self.click()
            self.invoke("type_text", "alpha")
            self.expect_title("Input alpha")
            self.invoke("key", "Backspace")
            self.expect_title("Input alph")
        elif cid == "U15":
            self.click(60, 90)
            self.expect_title("Required blocked")
            self.check("invalid submission sends no form", not any(row["path"] == "/echo-get" for row in self.requests()), "no /echo-get", self.requests())
            self.click()
            self.invoke("type_text", "AB12")
            self.click(60, 90)
            self.expect_title("EchoGET")
            self.expect_request("/echo-get", values={"q": "AB12", "case": "U15"})
        elif cid in ("U16", "U17"):
            text = "toy browser" if cid == "U16" else "post value"
            self.click()
            self.invoke("type_text", text)
            self.click(60, 90)
            self.expect_title("EchoGET" if cid == "U16" else "EchoPOST")
            self.expect_request("/echo-get" if cid == "U16" else "/echo-post", "GET" if cid == "U16" else "POST", {"q": text, "case": cid})
        elif cid == "U18":
            self.click()
            self.invoke("key", "ArrowDown")
            self.invoke("key", "Enter")
            self.expect_title("Select b")
            self.shot("selection-after-keyboard")
            self.click(60, 90)
            self.expect_title("EchoGET")
            self.expect_request("/echo-get", values={"pick": "b", "case": "U18"})
        elif cid == "U19":
            self.click()
            self.expect_title("Timer done", 4)
        elif cid == "U20":
            self.click()
            self.expect_title("Fetch FETCHED")
            self.expect_request("/api/value.txt")
        elif cid == "U21":
            self.navigate("/redirect")
            self.expect_title("Target")
            self.expect_url("/target.html")
            self.expect_request("/redirect")
            self.expect_request("/target.html")
        elif cid == "U22":
            self.navigate("/not-found")
            end = time.monotonic() + 6
            while time.monotonic() < end and "404" not in self.title():
                time.sleep(0.05)
            self.check("visible HTTP404 feedback", "404" in self.title(), "caption/document title contains 404", self.title())
            self.observe("HTTP404 body versus retained page")
            self.expect_request("/not-found")
        elif cid == "U23":
            unavailable = UnavailableEndpoint()
            try:
                try:
                    self.invoke("navigate", unavailable.url)
                except Exception as error:
                    self.observations.append({"label": "expected unavailable navigation exception", "actual": repr(error)})
                time.sleep(0.3)
                actual = self.observe("unavailable navigation feedback")
                raw_actions = getattr(self.adapter, "actions", [])
                error = getattr(self.adapter, "last_navigation_error", None)
                feedback = "Could not open" in actual["title"] or "ERR_" in json.dumps(raw_actions[-3:]) or bool(error)
                self.check("unavailable navigation has observable failure", feedback, "GUI error or actual reference navigation error", {"caption": actual["title"], "navigation_error": error, "adapter_actions": raw_actions[-3:]})
                self.navigate("/target.html")
                self.expect_title("Target")
                self.expect_request("/target.html")
            finally:
                unavailable.close()
        elif cid == "U25":
            self.click(12, 12)
            self.expect_title("Check on")
            self.invoke("key", "Space")
            self.expect_title("Check off")
        elif cid == "U26":
            self.invoke("scroll", -20)
            time.sleep(0.3)
            self.shot("scrolled-bottom-link")
            self.click(60, 480)
            self.expect_title("Target")
            self.expect_request("/target.html")
        self.observe("final")
        self.shot("final")
        status = "FAIL" if any(not item["passed"] for item in self.checks) else "PARTIAL" if self.limitations else "PASS"
        return status, None

    def run(self):
        self.output.mkdir(parents=True, exist_ok=False)
        status, error = "FAIL", None
        try:
            status, error = self.execute()
        except CapabilityUnavailable as caught:
            status, error = "BLOCKED", str(caught)
        except Exception as caught:
            status, error = "FAIL", repr(caught)
            (self.output / "exception.txt").write_text(traceback.format_exc(), encoding="utf-8")
            if self.adapter:
                try:
                    self.observe("exception")
                    self.shot("exception")
                except Exception:
                    pass
        finally:
            if self.adapter:
                try:
                    self.adapter.close()
                except Exception as caught:
                    self.limitations.append("Owned adapter cleanup failed: " + repr(caught))
                    status = "FAIL"
        capabilities = dict(getattr(self.adapter, "capabilities", {})) if self.adapter else {}
        capabilities.setdefault("mode", getattr(self.adapter, "mode", "not launched"))
        capabilities.setdefault("native_ime_composition", False)
        result = {**self.spec, "status": status, "error": error, "executed": self.adapter is not None,
                  "capabilities": capabilities, "fixture_sha256": self.server.manifest["files_sha256"],
                  "actions": self.actions, "adapter_actions": getattr(self.adapter, "actions", []),
                  "checks": self.checks, "observations": self.observations, "requests": self.requests(),
                  "limitations": self.limitations, "elapsed_seconds": round(time.monotonic() - self.started, 4)}
        save_json(self.output / "result.json", result)
        return result


def run_suite(adapter_factory, output, provenance, selected=None):
    output = Path(output).resolve()
    output.mkdir(parents=True, exist_ok=True)
    if (output / "results.json").exists():
        raise FileExistsError("Preserve the previous run; use a fresh output directory: " + str(output))
    server = FixtureServer(output / "corpus").start()
    results = []
    report = {"schema": "toy-browser-real-usability-v1", "provenance": provenance,
              "corpus_source_sha256": digest(Path(__file__).with_name("usability_corpus.py")),
              "runner_source_sha256": digest(__file__), "fixture_manifest": server.manifest,
              "safety": "Authored trusted file/loopback pages only; no arbitrary-web safety claim", "cases": results}
    try:
        for spec in server.manifest["cases"]:
            if selected and spec["id"] not in selected:
                continue
            result = Case(spec, server, adapter_factory, output / spec["id"]).run()
            results.append(result)
            print(result["id"], result["name"], result["status"], flush=True)
            report["status"] = dict(Counter(row["status"] for row in results))
            report["executed"] = sum(row["executed"] for row in results)
            save_json(output / "results.json", report)
    finally:
        server.close()
    print("SUMMARY", json.dumps({"status": report.get("status"), "executed": report.get("executed")}), flush=True)
    return report
