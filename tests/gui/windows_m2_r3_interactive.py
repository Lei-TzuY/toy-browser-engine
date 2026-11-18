#!/usr/bin/env python3
"""M2-R3 Windows Interactive & Pre-Flight Acceptance Harness.

In an isolated non-interactive session (such as `WinSta0\\exebox-*` while the
Owner holds `WinSta0\\Default`), this tool refuses to cross the desktop
boundary, records the raw `BLOCKED` (`WINDOWS_NATIVE_INPUT_HOLD`) receipt, and
performs non-input window-only rendering verification of the M2-R3 fixture page.

When launched by the Owner inside an interactive `WinSta0\\Default` session:
  python tests/gui/windows_m2_r3_interactive.py --interactive

it starts a loopback HTTP server with a single-page M2-R3 verification workbench
(covering P0 Alt/keyboard/resize/fast-click, P1 Chinese Bopomofo IME preedit,
Esc-cancel & commit, and P2 Unicode clipboard Ctrl+A/C/X/V & selection), opens
`browser_engine.exe --window`, records live HTTP submission receipts and window
screenshots, and writes `results.json` + `ACCEPTANCE_SUMMARY.md`.
"""

from __future__ import annotations

import argparse
import ctypes as c
from ctypes import wintypes as w
import hashlib
import html
import http.server
import json
from pathlib import Path
import struct
import subprocess
import sys
import threading
import time
import urllib.parse

REPO_ROOT = Path(__file__).resolve().parents[2]


M2_R3_WORKBENCH_HTML = """<!doctype html>
<html lang="zh-Hant">
<head>
  <meta charset="utf-8">
  <title>M2-R3 Windows Input Workbench</title>
  <style>
    body { font-family: sans-serif; margin: 16px; background: #f8fafc; color: #0f172a; }
    h1 { font-size: 20px; margin: 0 0 8px 0; }
    .card { background: #ffffff; border: 1px solid #cbd5e1; padding: 12px; margin-bottom: 12px; }
    label { display: block; font-weight: bold; margin-bottom: 4px; }
    input, textarea, select, button { font-size: 16px; padding: 6px; margin-bottom: 8px; }
    input[type="text"], input[type="password"], textarea { width: 460px; }
    #status { font-weight: bold; color: #1d4ed8; }
  </style>
</head>
<body>
  <h1>M2-R3 Windows 原生輸入驗收工作台 (P0 / P1 / P2)</h1>
  <p id="status">Ready — 請依序測試注音 IME、剪貼簿 (Ctrl+A/C/X/V)、Alt 導覽與視窗縮放</p>

  <form id="m2r3-form" method="POST" action="/submit">
    <div class="card">
      <label for="ime_field">1. P1 中文注音 IME 欄位 (輸入「中文測試😀」，測試組字底線、Esc 取消組字不關窗、Enter 上屏):</label>
      <input id="ime_field" name="ime_field" type="text" placeholder="在此輸入中文注音...">
      <p id="ime_log">input events: 0 | value: </p>
    </div>

    <div class="card">
      <label for="clip_source">2. P2 剪貼簿來源 (選取或 Ctrl+A 後按 Ctrl+C / Ctrl+X):</label>
      <input id="clip_source" name="clip_source" type="text" value="台灣瀏覽器 Copy/Cut 測試 🌐">
      <label for="clip_target">3. P2 剪貼簿目標 Textarea (按 Ctrl+V 貼上，支援 Shift+方向鍵與滑鼠拖曳選取):</label>
      <textarea id="clip_target" name="clip_target" rows="3"></textarea>
      <label for="pwd_field">4. P2 密碼欄位保護 (Ctrl+C / Ctrl+X 不得複製出密碼):</label>
      <input id="pwd_field" type="password" value="never-leak-password">
    </div>

    <div class="card">
      <label for="pick">5. Select 鍵盤操作 (ArrowDown 選 Beta):</label>
      <select id="pick" name="pick">
        <option value="a" selected>Alpha</option>
        <option value="b">Beta</option>
        <option value="c">Gamma</option>
      </select>
      <button id="fast_btn" type="button">快速點擊計數 (0)</button>
      <input id="clicks" name="clicks" type="hidden" value="0">
      <button id="submit_btn" type="submit">提交驗收收據 (POST /submit)</button>
    </div>
  </form>

  <script>
    var imeCount = 0;
    var imeField = document.getElementById("ime_field");
    var imeLog = document.getElementById("ime_log");
    imeField.addEventListener("input", function() {
      imeCount += 1;
      imeLog.textContent = "input events: " + imeCount + " | value: " + imeField.value;
    });

    var clickCount = 0;
    var fastBtn = document.getElementById("fast_btn");
    var clicksInput = document.getElementById("clicks");
    fastBtn.addEventListener("click", function() {
      clickCount += 1;
      clicksInput.value = String(clickCount);
      fastBtn.textContent = "快速點擊計數 (" + clickCount + ")";
    });
  </script>
</body>
</html>
"""


class WorkbenchServer:
    def __init__(self) -> None:
        self.receipts: list[dict[str, object]] = []
        self._httpd: http.server.ThreadingHTTPServer | None = None
        self._thread: threading.Thread | None = None
        self.port: int = 0

    def start(self) -> str:
        parent = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, format: str, *args: object) -> None:
                return

            def do_GET(self) -> None:
                body = M2_R3_WORKBENCH_HTML.encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_POST(self) -> None:
                if self.path != "/submit":
                    self.send_error(404)
                    return
                length = int(self.headers.get("Content-Length", "0"))
                if length < 0 or length > 16384:
                    self.send_error(413)
                    return
                raw = self.rfile.read(length).decode("utf-8", errors="replace")
                parsed = urllib.parse.parse_qs(raw, keep_blank_values=True)
                # No raw POST body on disk: it can contain secrets even when
                # the test page did not intend to submit them. Password input
                # values are never retained in receipts.
                def single(key: str) -> str | None:
                    values = parsed.get(key)
                    return values[0] if values and len(values) == 1 else None

                # Receipt values are strictly allowlisted. Free-form IME and
                # clipboard text can be private, so only record lengths and
                # assertions against the workbench's fixed test sentences.
                selected = single("pick")
                clicks = single("clicks")
                safe_fields: dict[str, str] = {}
                if selected is not None:
                    safe_fields["pick"] = selected if selected in ("a", "b", "c") else "INVALID"
                if clicks is not None:
                    safe_fields["clicks"] = clicks if clicks.isdecimal() and len(clicks) <= 6 else "INVALID"
                text_keys = ("ime_field", "clip_source", "clip_target")
                text_lengths = {key: len(single(key) or "") for key in text_keys if key in parsed}
                target = single("clip_target")
                source = single("clip_source")
                checks = {
                    "ime_expected_text": single("ime_field") == "中文測試😀",
                    "clipboard_expected_text": target == "台灣瀏覽器 Copy/Cut 測試 🌐",
                    "clipboard_matches_current_source": bool(target) and target == source,
                    "select_beta": selected == "b",
                }
                known = set(text_keys) | {"pwd_field", "pick", "clicks"}
                receipt = {
                    "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                    "path": self.path,
                    "field_names": sorted(set(parsed) & known),
                    "unknown_field_count": len(set(parsed) - known),
                    "redacted_fields": sorted((set(parsed) & known) - set(safe_fields)),
                    "fields": safe_fields,
                    "text_lengths": text_lengths,
                    "checks": checks,
                }
                parent.receipts.append(receipt)
                resp = (
                    "<!doctype html><meta charset='utf-8'>"
                    "<title>SUBMITTED_OK</title>"
                    "<h1>M2-R3 Receipt Recorded</h1>"
                    f"<pre id='receipt'>{html.escape(json.dumps(receipt, ensure_ascii=False, indent=2))}</pre>"
                    "<p><a href='/'>返回工作台</a></p>"
                ).encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "text/html; charset=utf-8")
                self.send_header("Content-Length", str(len(resp)))
                self.end_headers()
                self.wfile.write(resp)

        # Binding once avoids a release-and-rebind port race.
        self._httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.port = self._httpd.server_address[1]
        self._thread = threading.Thread(target=self._httpd.serve_forever, daemon=True)
        self._thread.start()
        return f"http://127.0.0.1:{self.port}/"

    def stop(self) -> None:
        if self._httpd is not None:
            self._httpd.shutdown()
            self._httpd.server_close()


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def _user_object_name(user: c.WinDLL, handle: int) -> str:
    UOI_NAME = 2
    needed = w.DWORD(0)
    user.GetUserObjectInformationW(w.HANDLE(handle), UOI_NAME, None, 0, c.byref(needed))
    if needed.value == 0:
        return ""
    buf = c.create_unicode_buffer(needed.value // 2 + 2)
    if user.GetUserObjectInformationW(
        w.HANDLE(handle), UOI_NAME, buf, needed.value, c.byref(needed)
    ):
        return buf.value
    return ""


def preflight_desktop() -> dict[str, object]:
    user = c.WinDLL("user32", use_last_error=True)
    kernel = c.WinDLL("kernel32", use_last_error=True)
    user.GetProcessWindowStation.restype = w.HANDLE
    user.GetThreadDesktop.argtypes = [w.DWORD]
    user.GetThreadDesktop.restype = w.HANDLE
    user.OpenInputDesktop.argtypes = [w.DWORD, w.BOOL, w.DWORD]
    user.OpenInputDesktop.restype = w.HANDLE
    user.CloseDesktop.argtypes = [w.HANDLE]
    user.GetForegroundWindow.restype = w.HWND
    user.GetCursorPos.argtypes = [c.POINTER(w.POINT)]
    user.GetCursorPos.restype = w.BOOL

    winsta = _user_object_name(user, user.GetProcessWindowStation())
    thread_desk_h = user.GetThreadDesktop(kernel.GetCurrentThreadId())
    thread_desk = _user_object_name(user, thread_desk_h)
    input_desk_h = user.OpenInputDesktop(0, False, 0x0001)
    input_desk = _user_object_name(user, input_desk_h) if input_desk_h else ""
    if input_desk_h:
        user.CloseDesktop(input_desk_h)

    fg = user.GetForegroundWindow()
    pt = w.POINT()
    cursor_ok = bool(user.GetCursorPos(c.byref(pt)))
    cursor_err = 0 if cursor_ok else c.get_last_error()

    same_desktop = bool(thread_desk and input_desk and thread_desk.lower() == input_desk.lower())
    ready = same_desktop and cursor_ok and bool(fg)
    hold_code = None if ready else "WINDOWS_NATIVE_INPUT_HOLD"
    reason = (
        "Interactive desktop owned and ready."
        if ready
        else (
            f"Process runs in '{winsta}\\{thread_desk}' while active input desktop is "
            f"'{winsta}\\{input_desk or 'unknown'}' (GetForegroundWindow={fg}, "
            f"GetCursorPos={cursor_ok}, winerr={cursor_err}). Refusing cross-desktop injection."
        )
    )
    return {
        "window_station": winsta,
        "thread_desktop": thread_desk,
        "input_desktop": input_desk,
        "foreground_hwnd": int(fg) if fg else None,
        "cursor_accessible": cursor_ok,
        "cursor_winerror": cursor_err,
        "ready": ready,
        "hold_code": hold_code,
        "reason": reason,
    }


def find_process_window(pid: int, timeout: float = 10.0) -> int | None:
    user = c.WinDLL("user32", use_last_error=True)
    enum_cb = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
    user.EnumWindows.argtypes = [enum_cb, w.LPARAM]
    user.IsWindowVisible.argtypes = [w.HWND]
    user.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        found: list[int] = []

        @enum_cb
        def _visit(hwnd: int, _lparam: int) -> bool:
            if not user.IsWindowVisible(hwnd):
                return True
            owner = w.DWORD(0)
            user.GetWindowThreadProcessId(hwnd, c.byref(owner))
            if owner.value == pid:
                found.append(int(hwnd))
                return False
            return True

        user.EnumWindows(_visit, 0)
        if found:
            return found[0]
        time.sleep(0.05)
    return None


def get_window_title(hwnd: int) -> str:
    user = c.WinDLL("user32", use_last_error=True)
    user.GetWindowTextW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
    buf = c.create_unicode_buffer(2048)
    user.GetWindowTextW(hwnd, buf, len(buf))
    return buf.value


def get_client_size(hwnd: int) -> tuple[int, int]:
    user = c.WinDLL("user32", use_last_error=True)
    user.GetClientRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
    rect = w.RECT()
    user.GetClientRect(hwnd, c.byref(rect))
    return int(rect.right - rect.left), int(rect.bottom - rect.top)


def capture_client_png(hwnd: int, path: Path) -> bool:
    # GUI imaging is optional for network-only receipt safety tests on CI.
    from PIL import Image

    user = c.WinDLL("user32", use_last_error=True)
    gdi = c.WinDLL("gdi32", use_last_error=True)
    user.GetClientRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
    user.GetDC.argtypes, user.GetDC.restype = [w.HWND], w.HDC
    user.ReleaseDC.argtypes = [w.HWND, w.HDC]
    gdi.CreateCompatibleDC.argtypes, gdi.CreateCompatibleDC.restype = [w.HDC], w.HDC
    gdi.CreateCompatibleBitmap.argtypes = [w.HDC, c.c_int, c.c_int]
    gdi.CreateCompatibleBitmap.restype = w.HBITMAP
    gdi.SelectObject.argtypes, gdi.SelectObject.restype = [w.HDC, w.HANDLE], w.HANDLE
    gdi.BitBlt.argtypes = [
        w.HDC, c.c_int, c.c_int, c.c_int, c.c_int, w.HDC, c.c_int, c.c_int, w.DWORD
    ]
    gdi.GetDIBits.argtypes = [
        w.HDC, w.HBITMAP, w.UINT, w.UINT, c.c_void_p, c.c_void_p, w.UINT
    ]
    gdi.DeleteObject.argtypes, gdi.DeleteDC.argtypes = [w.HANDLE], [w.HDC]

    width, height = get_client_size(hwnd)
    if width <= 0 or height <= 0:
        return False
    dc = user.GetDC(hwnd)
    memory = gdi.CreateCompatibleDC(dc)
    bitmap = gdi.CreateCompatibleBitmap(dc, width, height)
    old = gdi.SelectObject(memory, bitmap)
    try:
        if not gdi.BitBlt(memory, 0, 0, width, height, dc, 0, 0, 0x00CC0020):
            return False
        gdi.SelectObject(memory, old)
        header = struct.pack(
            "<IiiHHIIiiII", 40, width, height, 1, 32, 0, width * height * 4, 0, 0, 0, 0
        )
        info = c.create_string_buffer(header + bytes(16))
        pixels = c.create_string_buffer(width * height * 4)
        if gdi.GetDIBits(memory, bitmap, 0, height, pixels, info, 0) != height:
            return False
        bmp_path = path.with_suffix(".bmp")
        bmp_path.write_bytes(
            struct.pack("<2sIHHI", b"BM", 54 + len(pixels), 0, 0, 54) + header + pixels.raw
        )
        with Image.open(bmp_path) as img:
            img.save(path)
        bmp_path.unlink(missing_ok=True)
        return path.exists()
    finally:
        gdi.DeleteObject(bitmap)
        gdi.DeleteDC(memory)
        user.ReleaseDC(hwnd, dc)


def close_window(hwnd: int) -> None:
    user = c.WinDLL("user32", use_last_error=True)
    user.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
    user.PostMessageW(hwnd, 0x0010, 0, 0)  # WM_CLOSE


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        type=Path,
        default=REPO_ROOT / "target" / "release" / "browser_engine.exe",
        help="Path to browser_engine.exe",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=REPO_ROOT / "scratch" / "m2_r3" / "phase_c" / "m2_r3_workbench",
        help="Output evidence directory",
    )
    parser.add_argument(
        "--interactive",
        action="store_true",
        help="Keep the workbench window open for interactive Owner testing in WinSta0\\Default",
    )
    args = parser.parse_args()

    out_dir: Path = args.output.resolve()
    if out_dir.exists():
        parser.error("Evidence output already exists; choose a fresh directory")
    out_dir.mkdir(parents=True, exist_ok=False)
    binary: Path = args.binary.resolve()
    if not binary.exists():
        print(f"ERROR: binary not found: {binary}", file=sys.stderr)
        return 2

    preflight = preflight_desktop()
    server = WorkbenchServer()
    url = server.start()

    result: dict[str, object] = {
        "binary": str(binary),
        "binary_sha256": sha256_of(binary),
        "workbench_url": url,
        "preflight": preflight,
        "mode": "interactive" if args.interactive else "preflight_and_render",
        "native_input_status": "READY" if preflight.get("ready") else "BLOCKED",
        "hold_code": preflight.get("hold_code"),
        "hold_reason": preflight.get("reason"),
        "render_check": None,
        "http_receipts": [],
    }

    proc = subprocess.Popen(
        [str(binary), "--window", "--quiet", url],
        cwd=str(REPO_ROOT),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    try:
        hwnd = find_process_window(proc.pid, timeout=10.0)
        if hwnd:
            time.sleep(1.0)
            shot_path = out_dir / "workbench_window.png"
            captured = capture_client_png(hwnd, shot_path)
            result["render_check"] = {
                "hwnd": hex(hwnd),
                "pid": proc.pid,
                "title": get_window_title(hwnd),
                "client_size": list(get_client_size(hwnd)),
                "screenshot": str(shot_path) if captured else None,
                "render_pass": bool(captured and shot_path.exists()),
            }
        if args.interactive:
            print(f"Workbench running at {url} (PID {proc.pid}).")
            print("Complete your interactive checks in the browser window, then close the window.")
            proc.wait()
        else:
            if hwnd:
                close_window(hwnd)
            try:
                proc.wait(timeout=5.0)
            except subprocess.TimeoutExpired:
                proc.terminate()
    finally:
        if proc.poll() is None:
            proc.terminate()
        server.stop()

    result["http_receipts"] = server.receipts
    (out_dir / "results.json").write_text(
        json.dumps(result, ensure_ascii=False, indent=2),
        encoding="utf-8",
    )
    summary_md = f"""# M2-R3 Windows Interactive & Preflight Receipt

- **Binary**: `{result['binary']}`
- **Binary SHA-256**: `{result['binary_sha256']}`
- **Mode**: `{result['mode']}`
- **Native Input Status**: `{result['native_input_status']}`
- **Hold Code**: `{result['hold_code']}`
- **Reason**: {result['hold_reason']}
- **Render Check**: `{json.dumps(result['render_check'], ensure_ascii=False)}`
- **HTTP Receipts**: `{len(server.receipts)}`

## One-Shot Command for Owner's Interactive `WinSta0\\Default` Desktop

```powershell
python tests/gui/windows_m2_r3_interactive.py --interactive --binary .\\target\\release\\browser_engine.exe --output scratch\\m2_r3\\owner_interactive_run
```
"""
    (out_dir / "ACCEPTANCE_SUMMARY.md").write_text(summary_md, encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
