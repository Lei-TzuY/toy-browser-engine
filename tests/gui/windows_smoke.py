#!/usr/bin/env python3
"""Native Windows GUI smoke using a private loopback server and SendInput.

Run: python tests/gui/windows_smoke.py path/to/browser_engine.exe output-dir
Only windows belonging to the child browser process are targeted. A foreground
check precedes every key-down. This is synthetic keyboard evidence, not an IME,
accessibility, clipboard, or physical-hardware certification.
"""
import argparse
import ctypes as c
from ctypes import wintypes as w
import hashlib
import http.server
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import threading
import time
import urllib.parse


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--latin-input", action="store_true",
                        help="request an already loaded US English layout for the test window only")
    parser.add_argument("--restore-layout", type=lambda value: int(value, 0),
                        help="restore a previously recorded input layout before closing each test window")
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.error("requires native Windows; WSL is not Windows GUI evidence")
    binary = args.binary.resolve()
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=True)
    user, gdi = c.WinDLL("user32", use_last_error=True), c.WinDLL("gdi32", use_last_error=True)
    user.GetForegroundWindow.restype = w.HWND
    user.SetForegroundWindow.argtypes = [w.HWND]
    user.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
    user.GetWindowTextW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
    user.ShowWindow.argtypes = [w.HWND, c.c_int]
    user.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
    user.MapVirtualKeyW.argtypes = [w.UINT, w.UINT]
    user.VkKeyScanW.argtypes = [w.WCHAR]
    user.VkKeyScanW.restype = c.c_short
    user.GetKeyboardLayout.argtypes, user.GetKeyboardLayout.restype = [w.DWORD], w.HANDLE
    user.VkKeyScanExW.argtypes, user.VkKeyScanExW.restype = [w.WCHAR, w.HANDLE], c.c_short
    user.GetKeyboardLayoutList.argtypes, user.GetKeyboardLayoutList.restype = [c.c_int, c.POINTER(w.HANDLE)], c.c_int
    ime = c.WinDLL("imm32", use_last_error=True)
    ime.ImmGetContext.argtypes, ime.ImmGetContext.restype = [w.HWND], w.HANDLE
    ime.ImmGetOpenStatus.argtypes, ime.ImmGetOpenStatus.restype = [w.HANDLE], w.BOOL
    ime.ImmSetOpenStatus.argtypes, ime.ImmSetOpenStatus.restype = [w.HANDLE, w.BOOL], w.BOOL
    ime.ImmReleaseContext.argtypes = [w.HWND, w.HANDLE]
    contexts, diagnostics, requests = [], [], []

    class Mouse(c.Structure):
        _fields_ = [("dx", w.LONG), ("dy", w.LONG), ("data", w.DWORD),
                    ("flags", w.DWORD), ("time", w.DWORD), ("extra", c.c_size_t)]

    class Keyboard(c.Structure):
        _fields_ = [("vk", w.WORD), ("scan", w.WORD), ("flags", w.DWORD),
                    ("time", w.DWORD), ("extra", c.c_size_t)]

    class Event(c.Union):
        _fields_ = [("mouse", Mouse), ("keyboard", Keyboard)]

    class Input(c.Structure):
        _fields_ = [("kind", w.DWORD), ("event", Event)]

    user.SendInput.argtypes = [w.UINT, c.POINTER(Input), c.c_int]
    user.SendInput.restype = w.UINT
    enum_callback = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
    user.EnumWindows.argtypes = [enum_callback, w.LPARAM]
    user.IsWindowVisible.argtypes = [w.HWND]

    def wait(predicate, description, timeout=10):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            value = predicate()
            if value:
                return value
            time.sleep(0.03)
        raise AssertionError("timed out: " + description)

    def title(hwnd):
        buf = c.create_unicode_buffer(8192)
        user.GetWindowTextW(hwnd, buf, len(buf))
        return buf.value

    def focus(hwnd):
        user.ShowWindow(hwnd, 9)
        user.SetForegroundWindow(hwnd)
        wait(lambda: user.GetForegroundWindow() == hwnd, "foreground belongs to test browser", 3)

    def send(hwnd, vk, down=True, unicode=False):
        if down and user.GetForegroundWindow() != hwnd:
            raise AssertionError("refusing keyboard injection into another foreground window")
        item = Input()
        item.kind = 1
        scan = vk if unicode else user.MapVirtualKeyW(vk, 4)  # MAPVK_VK_TO_VSC_EX
        flags = (4 if unicode else 8) | (0 if down else 2)
        if not unicode and (scan & 0xff00 or vk in (0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27, 0x28, 0x2d, 0x2e)):
            flags |= 1  # KEYEVENTF_EXTENDEDKEY: Home must not become keypad 7
        item.event.keyboard = Keyboard(0, scan if unicode else scan & 0xff, flags, 0, 0)
        if user.SendInput(1, c.byref(item), c.sizeof(item)) != 1:
            raise c.WinError(c.get_last_error())

    def key(hwnd, vk, hold=0.045):
        send(hwnd, vk)
        time.sleep(hold)
        send(hwnd, vk, False)
        time.sleep(hold)

    def chord(hwnd, *keys):
        for vk in keys:
            send(hwnd, vk)
        time.sleep(0.12)
        for vk in reversed(keys):
            send(hwnd, vk, False)
        time.sleep(0.12)

    def text(hwnd, value):
        layout = user.GetKeyboardLayout(user.GetWindowThreadProcessId(hwnd, None))
        for char in value:
            mapped = user.VkKeyScanExW(char, layout)
            if mapped == -1 or (mapped >> 8) & ~1:
                raise AssertionError("ASCII layout mapping unavailable for " + repr(char))
            shift = bool(mapped & 0x100)
            if shift:
                send(hwnd, 0x10)
            key(hwnd, mapped & 0xff, hold=0.02)
            if shift:
                send(hwnd, 0x10, False)

    def screenshot(hwnd, path):
        # Capture the test window's client area only, never the user's desktop.
        user.GetClientRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
        user.GetDC.argtypes, user.GetDC.restype = [w.HWND], w.HDC
        user.ReleaseDC.argtypes = [w.HWND, w.HDC]
        gdi.CreateCompatibleDC.argtypes, gdi.CreateCompatibleDC.restype = [w.HDC], w.HDC
        gdi.CreateCompatibleBitmap.argtypes = [w.HDC, c.c_int, c.c_int]
        gdi.CreateCompatibleBitmap.restype = w.HBITMAP
        gdi.SelectObject.argtypes, gdi.SelectObject.restype = [w.HDC, w.HANDLE], w.HANDLE
        gdi.BitBlt.argtypes = [w.HDC, c.c_int, c.c_int, c.c_int, c.c_int,
                              w.HDC, c.c_int, c.c_int, w.DWORD]
        gdi.GetDIBits.argtypes = [w.HDC, w.HBITMAP, w.UINT, w.UINT, c.c_void_p, c.c_void_p, w.UINT]
        gdi.DeleteObject.argtypes, gdi.DeleteDC.argtypes = [w.HANDLE], [w.HDC]
        rect = w.RECT()
        user.GetClientRect(hwnd, c.byref(rect))
        width, height = rect.right, rect.bottom
        dc = user.GetDC(hwnd)
        memory = gdi.CreateCompatibleDC(dc)
        bitmap = gdi.CreateCompatibleBitmap(dc, width, height)
        old = gdi.SelectObject(memory, bitmap)
        try:
            if not gdi.BitBlt(memory, 0, 0, width, height, dc, 0, 0, 0x00CC0020):
                raise c.WinError(c.get_last_error())
            gdi.SelectObject(memory, old)
            header = struct.pack("<IiiHHIIiiII", 40, width, height, 1, 32, 0,
                                 width * height * 4, 0, 0, 0, 0)
            info = c.create_string_buffer(header + bytes(16))
            pixels = c.create_string_buffer(width * height * 4)
            if gdi.GetDIBits(memory, bitmap, 0, height, pixels, info, 0) != height:
                raise c.WinError(c.get_last_error())
            path.write_bytes(struct.pack("<2sIHHI", b"BM", 54 + len(pixels), 0, 0, 54)
                             + header + pixels.raw)
        finally:
            gdi.DeleteObject(bitmap)
            gdi.DeleteDC(memory)
            user.ReleaseDC(hwnd, dc)

    home = """<html><head><title>NativeHome</title></head><body>
<p>Controlled native keyboard fixture</p><input id='entry'>
<script>var log = ''; document.body.addEventListener('keydown', function(e) {
log = log + e.key; document.querySelector('title').textContent = 'NativeKeys ' + log;
});</script></body></html>"""
    counts, slow_started, slow_finished, captures = {}, {}, {}, []

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *unused):
            pass

        def do_GET(self):
            parsed = urllib.parse.urlsplit(self.path)
            path = parsed.path
            requests.append(self.path)
            counts[path] = counts.get(path, 0) + 1
            if path.startswith("/slow/") and counts[path] == 2:
                slow_started[path].set()
                time.sleep(1.5)
                slow_finished[path].set()
            if path == "/capture":
                value = urllib.parse.parse_qs(parsed.query, keep_blank_values=True).get("value", [""])[0]
                captures.append(value)
                body = "<title>Captured</title><p>Captured fixture</p>"
            elif path == "/two":
                body = "<title>NativeTwo</title><p>Second controlled tab</p>"
            else:
                body = home
            data = body.encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html;charset=utf-8")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    base = "http://127.0.0.1:" + str(server.server_port)
    results = []

    def record(name, **details):
        results.append({"case": name, "passed": True, **details})
        print("PASS", name, details, flush=True)

    def launch(path, label):
        log = (out / (label + ".log")).open("w", encoding="utf-8")
        proc = subprocess.Popen([str(binary), "--window", "--quiet", base + path],
                                cwd=out, stdout=log, stderr=subprocess.STDOUT)

        def find_window():
            found = []

            @enum_callback
            def callback(hwnd, unused):
                pid = w.DWORD()
                user.GetWindowThreadProcessId(hwnd, c.byref(pid))
                if pid.value == proc.pid and user.IsWindowVisible(hwnd):
                    found.append(hwnd)
                return True

            user.EnumWindows(callback, 0)
            return found[0] if found else None

        hwnd, original_layout = None, None
        try:
            hwnd = wait(find_window, "native window for PID " + str(proc.pid))
            focus(hwnd)
            original_layout = user.GetKeyboardLayout(user.GetWindowThreadProcessId(hwnd, None))
            if args.latin_input:
                layouts = (w.HANDLE * user.GetKeyboardLayoutList(0, None))()
                user.GetKeyboardLayoutList(len(layouts), layouts)
                english = next((layout for layout in layouts if layout & 0xffff == 0x0409), None)
                if english is None:
                    raise AssertionError("no loaded US English layout; test will not install or change host settings")
                user.PostMessageW(hwnd, 0x0050, 0, english)  # WM_INPUTLANGCHANGEREQUEST, own window only
                wait(lambda: user.GetKeyboardLayout(user.GetWindowThreadProcessId(hwnd, None)) & 0xffff == 0x0409,
                     "test window US English layout", 3)
            context = ime.ImmGetContext(hwnd)
            opened = bool(ime.ImmGetOpenStatus(context)) if context else None
            contexts.append({"original_thread_layout": hex(original_layout),
                             "thread_layout": hex(user.GetKeyboardLayout(user.GetWindowThreadProcessId(hwnd, None))),
                             "ime_open_before": opened,
                             "ime_open_after": bool(ime.ImmGetOpenStatus(context)) if context else None})
            if context:
                ime.ImmReleaseContext(hwnd, context)
            wait(lambda: title(hwnd).startswith("NativeHome"), "home fixture title")
            return proc, hwnd, log
        except BaseException:
            # launch() may fail before the caller receives its process handle.
            # Restore/close only this child even when no suitable layout exists.
            if hwnd and original_layout:
                user.PostMessageW(hwnd, 0x0050, 0, original_layout)
            if proc.poll() is None:
                proc.terminate()
                proc.wait(timeout=10)
            log.close()
            raise

    def finish(proc, hwnd, log):
        restore_layout(hwnd)
        user.PostMessageW(hwnd, 0x0010, 0, 0)  # graceful WM_CLOSE, no global keystrokes
        code = proc.wait(timeout=10)
        log.close()
        if code != 0:
            raise AssertionError("native application exit: " + str(code))

    def restore_layout(hwnd):
        restore = args.restore_layout or int(contexts[-1]["original_thread_layout"], 16)
        user.PostMessageW(hwnd, 0x0050, 0, restore)
        wait(lambda: user.GetKeyboardLayout(user.GetWindowThreadProcessId(hwnd, None)) == restore,
             "restore original input layout", 3)
        contexts[-1]["restored_layout"] = hex(restore)

    proc = None
    failure = None
    try:
        proc, hwnd, log = launch("/home", "tabs")
        screenshot(hwnd, out / "native-launch.bmp")
        record("native window launch", pid=proc.pid, title=title(hwnd))
        chord(hwnd, 0x11, ord("T"))
        diagnostics.append({"stage": "after Ctrl+T", "title": title(hwnd)})
        screenshot(hwnd, out / "after-new-tab.bmp")
        text(hwnd, base + "/two")
        diagnostics.append({"stage": "after address typing", "title": title(hwnd)})
        screenshot(hwnd, out / "after-address-typing.bmp")
        key(hwnd, 0x0d)
        wait(lambda: title(hwnd).startswith("NativeTwo"), "new tab address navigation")
        screenshot(hwnd, out / "native-two-tabs.bmp")
        record("Ctrl+T and address editing")
        chord(hwnd, 0x11, ord("1"))
        wait(lambda: title(hwnd).startswith("NativeHome"), "Ctrl+1 preserves other tab DOM")
        chord(hwnd, 0x11, 0x09)
        wait(lambda: title(hwnd).startswith("NativeTwo"), "Ctrl+Tab")
        record("Ctrl+1 and Ctrl+Tab routing")

        for vk in (0x11, 0x10, 0x09):
            send(hwnd, vk)
        time.sleep(0.12)
        send(hwnd, 0x11, False)
        time.sleep(0.10)
        send(hwnd, 0x10, False)
        time.sleep(0.10)
        send(hwnd, 0x09)  # explicit repeat with modifiers released
        time.sleep(0.12)
        send(hwnd, 0x09, False)
        wait(lambda: title(hwnd).startswith("NativeHome"), "staggered Ctrl+Shift+Tab")
        record("staggered Ctrl+Shift+Tab repeat does not reach DOM", title=title(hwnd))
        text(hwnd, "aw1b")
        wait(lambda: title(hwnd).startswith("NativeKeys aw1b "), "ordinary exact DOM input")
        record("ordinary letters and digits", expected="aw1b", actual=title(hwnd))
        chord(hwnd, 0x11, ord("9"))
        wait(lambda: title(hwnd).startswith("NativeTwo"), "Ctrl+9")
        chord(hwnd, 0x11, ord("W"))
        wait(lambda: title(hwnd).startswith("NativeKeys aw1b "), "Ctrl+W closes only active tab")
        record("Ctrl+9 and Ctrl+W preserve other tab input")
        finish(proc, hwnd, log)
        proc = None
        record("graceful native exit")

        for index, expected in enumerate(("aw1b", "www.example.com", "127.0.0.1", "localhost:8080")):
            path = "/slow/" + str(index)
            slow_started[path], slow_finished[path] = threading.Event(), threading.Event()
            proc, hwnd, log = launch(path, "slow-" + str(index))
            chord(hwnd, 0x11, ord("T"))
            if not slow_started[path].wait(5):
                raise AssertionError("new-tab slow request did not start")
            text(hwnd, expected)
            typed_during_load = not slow_finished[path].is_set()
            if not slow_finished[path].wait(5):
                raise AssertionError("slow request did not finish")
            time.sleep(0.2)
            key(hwnd, 0x24)  # Home, prepend capture URL to the actual editor contents
            text(hwnd, base + "/capture?value=")
            key(hwnd, 0x0d)
            wait(lambda: title(hwnd).startswith("Captured"), "address capture endpoint")
            actual = captures[-1]
            if actual != expected or not typed_during_load:
                raise AssertionError((expected, actual, typed_during_load))
            record("slow Ctrl+T exact input", expected=expected, actual=actual,
                   typed_during_load=typed_during_load)
            finish(proc, hwnd, log)
            proc = None
    except BaseException as error:
        failure = repr(error)
        if proc is not None and proc.poll() is None:
            diagnostics.append({"stage": "failure", "title": title(hwnd)})
            screenshot(hwnd, out / "failure.bmp")
        raise
    finally:
        if proc is not None and proc.poll() is None:
            restore_layout(hwnd)
            proc.terminate()
            proc.wait(timeout=10)
        server.shutdown()
        server.server_close()
        receipt = {
            "platform": sys.platform, "os_version": str(sys.getwindowsversion()),
            "binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "method": "real native window; synthetic Win32 SendInput scan codes; loopback fixtures",
            "latin_input_fixture": args.latin_input, "input_contexts": contexts,
            "diagnostics": diagnostics, "requests": requests, "failure": failure,
            "not_tested": ["live IME", "clipboard", "physical keyboard hardware", "arbitrary websites"],
            "checks": results, "passed": len(results), "completed": len(results) == 11 and failure is None,
        }
        (out / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    if len(results) != 11:
        raise AssertionError("incomplete Windows smoke")


if __name__ == "__main__":
    main()
