#!/usr/bin/env python3
"""Portable trusted-loopback corpus for actual M2-R2 browser interactions.

Importing this file does not start a server, write files or launch a browser.
materialize() and FixtureServer are shared by X11, native Windows and reference
adapters so every platform uses the same HTML bytes and endpoint semantics.
"""
import argparse
import hashlib
import http.server
import json
from pathlib import Path
import socket
import threading
import time
import urllib.parse


CSS = """html,body{margin:0;padding:0}body{font-size:18px;color:#111111;background-color:white}
.control{display:block;box-sizing:border-box;width:240px;height:52px;margin:0 0 12px 0;padding:0;font-size:18px}
p{margin:0;padding:0}form{margin:0;padding:0}a.control{background-color:#cc3333;color:white}
"""


def html(title, body, script="", css=""):
    body = body.replace('<input type="hidden"', '<input style="display:none" type="hidden"')
    script = "<script>" + script + "</script>" if script else ""
    return ("<!doctype html><html><head><meta charset=\"utf-8\"><title>" + title
            + "</title><style>" + CSS + css + "</style></head><body>"
            + body + script + "</body></html>")


def title_js(expression):
    return 'document.querySelector("title").textContent=' + expression + ";"


PAGES = {
    "index.html": html("Home", '<a id="go" class="control" href="target.html">Open target</a><p>Trusted interaction home</p>'),
    "target.html": html("Target", '<a id="home" class="control" href="index.html">Return home</a><p id="result">TARGET VISIBLE</p>'),
    "file.html": html("FileHome", '<a id="go" class="control" href="file-target.html">Open local target</a>'),
    "file-target.html": html("FileTarget", '<p id="result">LOCAL FILE TARGET</p>', css='p{height:80px;background-color:#0066cc;color:white}'),
    "color.html": html("Colors", '<div id="red"></div><div id="blue"></div>', css='#red{display:block;width:240px;height:100px;background:#cc3333}#blue{display:block;width:240px;height:100px;background:#3366cc}'),
    "unicode.html": html("Unicode", '<div id="latin">Latin: Café Ångström</div><div id="greek">Greek: Ελληνικά</div><div id="cjk">Chinese: 中文測試</div><div id="emoji">Emoji: 😀</div>', css='div{display:block;height:64px;font-size:28px}'),
    "image.html": html("Image", '<img id="image" src="tile.png" width="40" height="40" alt="magenta test tile"><p>PNG TILE</p>', css='img{display:block}'),
    "click.html": html("Click 0", '<button id="act" class="control" type="button">Change visible text</button><p id="out">WAITING</p>',
        'let count=0;document.getElementById("act").addEventListener("click",function(){count=count+1;document.getElementById("out").textContent="CLICKED "+count;' + title_js('"Click "+count') + '});'),
    "input.html": html("Input", '<input id="q" class="control" name="q"><p id="out">EMPTY</p>',
        'let q=document.getElementById("q");q.addEventListener("input",function(){document.getElementById("out").textContent=q.value;' + title_js('"Input "+q.value') + '});'),
    "tabstate.html": html("TabState", '<form action="/echo-get" method="get"><input id="q" class="control" name="q"><input type="hidden" name="case" value="U08"><button id="submit" class="control" type="submit">Capture tab value</button></form>',
        'let q=document.getElementById("q");q.addEventListener("input",function(){' + title_js('"TabState "+q.value') + '});'),
    "required.html": html("Required", '<form action="/echo-get" method="get"><input id="q" class="control" name="q" required pattern="[A-Z]{2}[0-9]{2}"><input type="hidden" name="case" value="U15"><button id="submit" class="control" type="submit">Submit required value</button></form>',
        'document.getElementById("q").addEventListener("invalid",function(){' + title_js('"Required blocked"') + '});'),
    "get.html": html("GetForm", '<form action="/echo-get" method="get"><input id="q" class="control" name="q"><input type="hidden" name="case" value="U16"><button id="submit" class="control" type="submit">Submit GET</button></form>'),
    "post.html": html("PostForm", '<form action="/echo-post" method="post"><input id="q" class="control" name="q"><input type="hidden" name="case" value="U17"><button id="submit" class="control" type="submit">Submit POST</button></form>'),
    "select.html": html("Select a", '<form action="/echo-get" method="get"><select id="pick" class="control" name="pick"><option value="a">Alpha</option><option value="b">Beta</option><option value="c">Gamma</option></select><input type="hidden" name="case" value="U18"><button id="submit" class="control" type="submit">Submit selected value</button></form>',
        'let pick=document.getElementById("pick");pick.addEventListener("change",function(){' + title_js('"Select "+pick.value') + '});'),
    "timer.html": html("Timer idle", '<button id="act" class="control" type="button">Start timer</button><p id="out">IDLE</p>',
        'document.getElementById("act").addEventListener("click",function(){document.getElementById("out").textContent="WAITING";' + title_js('"Timer waiting"') + 'setTimeout(function(){document.getElementById("out").textContent="TIMER DONE";' + title_js('"Timer done"') + '},250);});'),
    "fetch.html": html("Fetch idle", '<button id="act" class="control" type="button">Fetch local text</button><p id="out">IDLE</p>',
        'document.getElementById("act").addEventListener("click",function(){fetch("/api/value.txt").then(function(r){return r.text();}).then(function(value){document.getElementById("out").textContent=value;' + title_js('"Fetch "+value') + '}).catch(function(error){' + title_js('"Fetch failed "+error') + '});});'),
    "checkbox.html": html("Check off", '<input id="check" type="checkbox"><p id="out">OFF</p>',
        'let check=document.getElementById("check");check.addEventListener("change",function(){let state=check.checked?"on":"off";document.getElementById("out").textContent=state;' + title_js('"Check "+state') + '});', css='#check{display:block;width:24px;height:24px;margin:0}'),
    "scroll.html": html("Scroll", '<div id="spacer">TOP</div><a id="bottom" class="control" href="target.html">Bottom target</a><div id="tail">END</div>', css='#spacer{height:700px;background-color:#cc3333}#bottom{height:100px;margin:0;background-color:#3366cc}#tail{height:50px}'),
    "ime.html": html("IME", '<input id="q" class="control"><p>Requires actual operating-system composition.</p>',
        'let q=document.getElementById("q");q.addEventListener("input",function(){' + title_js('"IME "+q.value') + '});'),
}


def png_tile():
    """Original baseline bytes, independent of the host compression library."""
    return bytes.fromhex(
        "89504e470d0a1a0a0000000d4948445200000028000000280802000000039c2f3a"
        "0000003049444154789cedcd310d00000803b0dd88c0bf1464618284a749ff66aa5f"
        "442c168bc562b1582c168bc562b1582cbeb328addc4ca5ff27b20000000049454e44ae426082"
    )


CASE_SPECS = [
    ("U01", "File page and relative link", "file.html", "Click #go; live title FileTarget and file URL; visible local target"),
    ("U02", "Full HTTP address entry", "index.html", "Ctrl+L; type full loopback /target.html URL; Enter; live title Target and target request"),
    ("U03", "Mouse hyperlink", "index.html", "Click #go; live title Target; server receives /target.html"),
    ("U04", "Back", "index.html", "Click #go then Back; live Home title and original URL"),
    ("U05", "Forward", "index.html", "Click #go, Back, Forward; live Target title and target URL"),
    ("U06", "Reload", "/reload", "Reload button; /reload hit count increases exactly once and live Reload title changes"),
    ("U07", "New tab", "index.html", "Open a new tab; observe two tabs and initial destination; navigate the new tab to controlled Target"),
    ("U08", "Independent tab field state", "tabstate.html", "Type first; new tab type second; switch both; submit first and capture q=first"),
    ("U09", "Close active tab", "index.html", "Open second tab, navigate Target, Ctrl+W; one tab remains on Home"),
    ("U10", "Solid CSS rendering", "color.html", "Actual screenshot page pixels (20,20)=204,51,51 and (20,140)=51,102,204"),
    ("U11", "Unicode visible glyphs", "unicode.html", "Inspect actual screenshot Latin/Greek/Chinese/emoji glyphs against actual reference; DOM/title alone cannot pass"),
    ("U12", "PNG image", "image.html", "Actual page pixel (5,5)=200,10,30 and GET /tile.png observed"),
    ("U13", "JavaScript mouse event", "click.html", "Click #act; live title Click 1 and screenshot shows CLICKED 1"),
    ("U14", "Text input and editing", "input.html", "Type alpha in #q then Backspace; live title Input alph and visible field/readout"),
    ("U15", "Required validation", "required.html", "Empty submit fires invalid/live Required blocked with no echo request; type AB12 and submit successfully"),
    ("U16", "GET form submission", "get.html", "Type toy browser and click submit; live EchoGET and decoded server q=toy browser"),
    ("U17", "POST form submission", "post.html", "Type post value and click submit; live EchoPOST; decoded POST body q=post value"),
    ("U18", "Select keyboard control", "select.html", "Focus select; ArrowDown then Enter; live Select b; submit server pick=b"),
    ("U19", "Timer after click", "timer.html", "Click #act; actual event loop updates live title Timer done and visible readout within4seconds"),
    ("U20", "Fetch after click", "fetch.html", "Click #act; GET /api/value.txt; live title Fetch FETCHED and visible readout"),
    ("U21", "HTTP302 redirect", "index.html", "Navigate /redirect; /target.html final URL/live title; both request paths observed"),
    ("U22", "HTTP404 feedback", "index.html", "Navigate /not-found; visible 404 feedback captured; record whether response body is rendered or previous page retained"),
    ("U23", "Recovery after unavailable URL", "index.html", "Navigate owned bound-but-not-listening loopback port; record actual failure feedback; navigate valid target successfully"),
    ("U24", "Chinese operating-system IME composition", "ime.html", "Actual composition only; committed Unicode injection is distinct; BLOCKED if no supported IME composition adapter"),
    ("U25", "Checkbox mouse and keyboard", "checkbox.html", "Click checkbox then Space; live Check on then Check off; visible state changes"),
    ("U26", "Scroll and activate bottom link", "scroll.html", "Wheel down to bottom; screenshot shows link; click #bottom and reach live Target"),
]


def materialize(output):
    output = Path(output).resolve()
    site = output / "site"
    site.mkdir(parents=True, exist_ok=True)
    for name, contents in PAGES.items():
        (site / name).write_bytes(contents.encode("utf-8"))
    (site / "tile.png").write_bytes(png_tile())
    manifest = {
        "schema": "toy-browser-real-usability-v1",
        "viewport": {"width": 800, "height": 600, "toy_page_origin": [0, 68]},
        "coordinates": {"first_control": [60, 26], "submit": [60, 90], "checkbox": [12, 12], "bottom_link_after_scroll": [60, 480], "toolbar_back": [20, 50], "toolbar_forward": [52, 50], "toolbar_reload": [84, 50]},
        "files_sha256": {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(site.iterdir())},
        "cases": [{"id": case, "name": name, "route": route, "expected": expected, "required_capability": "os_ime_composition" if case == "U24" else "visible_glyph_review" if case == "U11" else "actual_window_interaction"} for case, name, route, expected in CASE_SPECS],
        "safety": "Trusted authored local/loopback pages only; no arbitrary-web safety claim.",
    }
    (output / "corpus-manifest.json").write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    return manifest


class UnavailableEndpoint:
    """Reserve a loopback port without listening; never touch an unrelated service."""
    def __init__(self):
        self.socket = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.socket.bind(("127.0.0.1", 0))
        self.url = "http://127.0.0.1:%d/unavailable" % self.socket.getsockname()[1]

    def close(self):
        self.socket.close()


class FixtureServer:
    def __init__(self, output, host="127.0.0.1", port=0):
        self.output = Path(output).resolve()
        self.manifest = materialize(self.output)
        self.site = self.output / "site"
        self._requests, self._lock = [], threading.Lock()
        self.started = time.monotonic()
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *unused):
                pass

            def do_GET(self):
                self.respond()

            def do_POST(self):
                self.respond()

            def respond(self):
                parsed = urllib.parse.urlsplit(self.path)
                body = self.rfile.read(min(int(self.headers.get("Content-Length", "0")), 65536))
                with owner._lock:
                    row = {"sequence": len(owner._requests) + 1, "at_seconds": round(time.monotonic() - owner.started, 6), "method": self.command, "path": parsed.path, "query": parsed.query, "body": body.decode("utf-8", errors="replace"), "content_type": self.headers.get("Content-Type")}
                    owner._requests.append(row)
                    with (owner.output / "requests.jsonl").open("a", encoding="utf-8") as log:
                        log.write(json.dumps(row, ensure_ascii=False) + "\n")
                    hit = sum(item["path"] == parsed.path for item in owner._requests)
                status, kind, headers = 200, "text/html;charset=utf-8", {}
                if parsed.path == "/redirect":
                    status, data, headers = 302, b"", {"Location": "/target.html"}
                elif parsed.path == "/not-found":
                    status, data = 404, html("NotFound404", '<p id="error">HTTP404: controlled missing page</p>', css='p{height:80px;background-color:#ffcccc}').encode()
                elif parsed.path == "/reload":
                    data = html("Reload " + str(hit), '<p id="hit">SERVER HIT ' + str(hit) + '</p>').encode()
                elif parsed.path in ("/echo-get", "/echo-post"):
                    title = "EchoGET" if self.command == "GET" else "EchoPOST"
                    data = html(title, '<p id="result">SERVER RECEIVED FORM</p>').encode()
                elif parsed.path == "/api/value.txt":
                    kind, data = "text/plain;charset=utf-8", b"FETCHED"
                else:
                    candidate = (owner.site / (urllib.parse.unquote(parsed.path).lstrip("/") or "index.html")).resolve()
                    if candidate.parent != owner.site or not candidate.is_file():
                        status, data = 404, b"controlled route not found"
                    else:
                        kind = "image/png" if candidate.suffix == ".png" else kind
                        data = candidate.read_bytes()
                self.send_response(status)
                self.send_header("Content-Type", kind)
                self.send_header("Content-Length", str(len(data)))
                self.send_header("Cache-Control", "no-store")
                for key, value in headers.items():
                    self.send_header(key, value)
                self.end_headers()
                try:
                    self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError):
                    pass

        self.server = http.server.ThreadingHTTPServer((host, port), Handler)
        self.base = "http://127.0.0.1:%d" % self.server.server_port
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def start(self):
        self.thread.start()
        return self

    def requests(self):
        with self._lock:
            return [dict(row) for row in self._requests]

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--materialize-only", action="store_true")
    args = parser.parse_args()
    if args.materialize_only:
        print(json.dumps(materialize(args.output), indent=2, ensure_ascii=False))
        return
    server = FixtureServer(args.output, port=args.port).start()
    print(json.dumps({"base": server.base, "manifest": str(args.output.resolve() / "corpus-manifest.json")}), flush=True)
    try:
        threading.Event().wait()
    except KeyboardInterrupt:
        pass
    finally:
        server.close()


if __name__ == "__main__":
    main()
