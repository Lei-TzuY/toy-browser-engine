"""Private test drivers: owned Win32 window and isolated Chrome CDP profile.

Importing this module does not launch a browser. Only trusted loopback/file
fixtures are intended. Native keys and committed Unicode are distinct from IME.
"""
import ctypes as c
from ctypes import wintypes as w
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import time
import urllib.request


def wait(predicate, description, timeout=8):
    until = time.monotonic() + timeout
    last = None
    while time.monotonic() < until:
        last = predicate()
        if last:
            return last
        time.sleep(0.04)
    raise AssertionError(f"Timed out: {description}; last={last!r}")


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


KEYS = {"Tab": 9, "Enter": 13, "Shift": 16, "Control": 17, "Alt": 18,
        "Escape": 27, "Space": 32, "PageUp": 33, "PageDown": 34,
        "End": 35, "Home": 36, "ArrowLeft": 37, "ArrowUp": 38,
        "ArrowRight": 39, "ArrowDown": 40, "Delete": 46, "Backspace": 8}


class WindowsToy:
    mode = "native-windows-gui"
    page_origin = (0, 68)
    capabilities = {"engine": "toy-browser", "platform": "native Windows",
                    "real_window": True, "native_ime_composition": False,
                    "screenshot_includes_chrome": True, "screenshot_region": "owned client",
                    "navigation_method": "actual Win32 keyboard and pointer input"}

    def __init__(self, binary, url, output):
        if sys.platform != "win32":
            raise RuntimeError("Native Windows only; WSL does not qualify")
        self.binary, self.output = Path(binary).resolve(), Path(output).resolve()
        self.output.mkdir(parents=True, exist_ok=True)
        self.capabilities = {**self.capabilities, "os_version": str(sys.getwindowsversion())}
        self.actions, self.held = [], set()
        self.user, self.gdi = c.WinDLL("user32", use_last_error=True), c.WinDLL("gdi32", use_last_error=True)
        u = self.user
        u.GetForegroundWindow.restype = w.HWND
        u.SetForegroundWindow.argtypes = [w.HWND]
        u.GetWindowThreadProcessId.argtypes = [w.HWND, c.POINTER(w.DWORD)]
        u.GetWindowTextW.argtypes = [w.HWND, w.LPWSTR, c.c_int]
        u.ShowWindow.argtypes = [w.HWND, c.c_int]
        u.PostMessageW.argtypes = [w.HWND, w.UINT, w.WPARAM, w.LPARAM]
        u.IsWindow.argtypes = [w.HWND]
        u.IsWindowVisible.argtypes = [w.HWND]
        u.GetKeyboardLayout.argtypes, u.GetKeyboardLayout.restype = [w.DWORD], w.HANDLE
        u.GetKeyboardLayoutList.argtypes = [c.c_int, c.POINTER(w.HANDLE)]
        u.VkKeyScanExW.argtypes, u.VkKeyScanExW.restype = [w.WCHAR, w.HANDLE], c.c_short
        u.MapVirtualKeyW.argtypes = [w.UINT, w.UINT]
        u.ClientToScreen.argtypes = [w.HWND, c.POINTER(w.POINT)]
        u.SetCursorPos.argtypes = [c.c_int, c.c_int]
        u.GetCursorPos.argtypes = [c.POINTER(w.POINT)]
        u.LogicalToPhysicalPointForPerMonitorDPI.argtypes = [w.HWND, c.POINTER(w.POINT)]
        u.SetPhysicalCursorPos.argtypes = [c.c_int, c.c_int]
        u.GetPhysicalCursorPos.argtypes = [c.POINTER(w.POINT)]
        u.WindowFromPhysicalPoint.argtypes, u.WindowFromPhysicalPoint.restype = [w.POINT], w.HWND
        u.BringWindowToTop.argtypes = [w.HWND]
        u.WindowFromPoint.argtypes, u.WindowFromPoint.restype = [w.POINT], w.HWND
        u.GetAncestor.argtypes, u.GetAncestor.restype = [w.HWND, w.UINT], w.HWND
        u.GetWindowDpiAwarenessContext.argtypes, u.GetWindowDpiAwarenessContext.restype = [w.HWND], w.HANDLE
        u.SetThreadDpiAwarenessContext.argtypes, u.SetThreadDpiAwarenessContext.restype = [w.HANDLE], w.HANDLE
        self.callback_type = c.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
        u.EnumWindows.argtypes = [self.callback_type, w.LPARAM]

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

        self.Mouse, self.Keyboard, self.Input = Mouse, Keyboard, Input
        u.SendInput.argtypes, u.SendInput.restype = [w.UINT, c.POINTER(Input), c.c_int], w.UINT
        self.log = (self.output / "browser.log").open("wb")
        self.proc = subprocess.Popen([str(self.binary), "--window", "--quiet", "--size", "800x600", str(url)],
                                     cwd=self.output, stdout=self.log, stderr=subprocess.STDOUT)
        self.hwnd, self.original_layout, self.original_dpi_context = None, None, None
        try:
            self.hwnd = wait(self._find_window, "owned browser window")
            context = u.GetWindowDpiAwarenessContext(self.hwnd)
            self.original_dpi_context = u.SetThreadDpiAwarenessContext(context)
            if not self.original_dpi_context:
                raise c.WinError(c.get_last_error())
            u.ShowWindow(self.hwnd, 9)
            u.BringWindowToTop(self.hwnd)
            u.SetForegroundWindow(self.hwnd)
            if u.GetForegroundWindow() != self.hwnd:
                # Windows may refuse foreground activation by a background
                # test launcher. Use a normal click only when the exact owned
                # window is already visible under a neutral chrome point.
                point = w.POINT(400, 16)
                u.ClientToScreen(self.hwnd, c.byref(point))
                if not u.LogicalToPhysicalPointForPerMonitorDPI(self.hwnd, c.byref(point)):
                    raise c.WinError(c.get_last_error())
                u.SetPhysicalCursorPos(point.x, point.y)
                if u.GetAncestor(u.WindowFromPhysicalPoint(point), 2) != self.hwnd:
                    raise RuntimeError("Owned activation point is obscured; no desktop input sent")
                actual = w.POINT()
                u.GetPhysicalCursorPos(c.byref(actual))
                if (actual.x, actual.y) != (point.x, point.y):
                    raise RuntimeError("Pointer moved before owned-window activation")
                for flags in (2, 4):
                    item = self.Input()
                    item.kind = 0
                    item.event.mouse = self.Mouse(0, 0, 0, flags, 0, 0)
                    self._input(item)
                    time.sleep(0.12)
                self.record("activate_owned_window_by_chrome_click", client=[400, 16])
            self._foreground()
            self.original_layout = self.layout()
            layouts = (w.HANDLE * u.GetKeyboardLayoutList(0, None))()
            u.GetKeyboardLayoutList(len(layouts), layouts)
            english = next((x for x in layouts if x & 0xffff == 0x0409), None)
            if english is None:
                raise RuntimeError("No already-loaded US English layout; fixture will not install one")
            self.set_layout(english)
            self.record("launch", pid=self.proc.pid, binary_sha256=digest(self.binary),
                        original_layout=hex(self.original_layout), fixture_layout=hex(self.layout()),
                        original_test_thread_dpi_context=hex(self.original_dpi_context),
                        matched_window_dpi_context=hex(context))
            self._first_painted_frame()
        except BaseException:
            self.close()
            raise

    def record(self, action, **data):
        self.actions.append({"action": action, "monotonic": time.monotonic(), **data})

    def _first_painted_frame(self):
        from PIL import Image
        path = self.output / "first-painted-frame.png"

        def painted():
            self.screenshot(path)
            with Image.open(path) as frame:
                return frame.size == (800, 668) and frame.convert("RGB").getpixel((799, 0)) == (222, 225, 230)

        wait(painted, "own client has painted browser chrome", 12)
        self.record("first_painted_frame", path=str(path), sha256=digest(path))

    def _find_window(self):
        found = []

        @self.callback_type
        def callback(hwnd, unused):
            pid = w.DWORD()
            self.user.GetWindowThreadProcessId(hwnd, c.byref(pid))
            if pid.value == self.proc.pid and self.user.IsWindowVisible(hwnd):
                found.append(hwnd)
            return True

        self.user.EnumWindows(callback, 0)
        return found[0] if found else None

    def _foreground(self):
        wait(lambda: self.user.GetForegroundWindow() == self.hwnd,
             "foreground belongs to owned browser", 3)

    def layout(self):
        return self.user.GetKeyboardLayout(self.user.GetWindowThreadProcessId(self.hwnd, None))

    def set_layout(self, layout):
        self.user.PostMessageW(self.hwnd, 0x0050, 0, layout)
        wait(lambda: self.layout() == layout, "own-window input layout", 3)

    def _input(self, item):
        if self.user.SendInput(1, c.byref(item), c.sizeof(item)) != 1:
            raise c.WinError(c.get_last_error())

    def _send(self, vk, down=True, unicode=False):
        if down:
            self._foreground()
        scan = vk if unicode else self.user.MapVirtualKeyW(vk, 4)
        flags = (4 if unicode else 8) | (0 if down else 2)
        if not unicode and (scan & 0xff00 or 33 <= vk <= 40 or vk in (45, 46)):
            flags |= 1
        item = self.Input()
        item.kind = 1
        item.event.keyboard = self.Keyboard(0, scan if unicode else scan & 0xff, flags, 0, 0)
        self._input(item)
        if not unicode:
            self.held.add(vk) if down else self.held.discard(vk)

    def key(self, name):
        vk = KEYS.get(name, ord(name.upper()) if len(name) == 1 else None)
        if vk is None:
            raise ValueError(name)
        self._send(vk)
        time.sleep(0.12)
        self._send(vk, False)
        self.record("key", key=name)
        time.sleep(0.04)

    def chord(self, *names):
        values = [KEYS.get(n, ord(n.upper()) if len(n) == 1 else None) for n in names]
        if None in values:
            raise ValueError(names)
        try:
            for value in values:
                self._send(value)
            time.sleep(0.12)
        finally:
            for value in reversed(values):
                if value in self.held:
                    self._send(value, False)
        self.record("chord", keys=list(names))
        time.sleep(0.08)

    def type_text(self, text):
        for char in text:
            if ord(char) > 127:
                for offset in range(0, len(char.encode("utf-16-le")), 2):
                    unit = int.from_bytes(char.encode("utf-16-le")[offset:offset + 2], "little")
                    self._send(unit, unicode=True)
                    self._send(unit, False, unicode=True)
            else:
                mapped = self.user.VkKeyScanExW(char, self.layout())
                if mapped == -1 or (mapped >> 8) & ~1:
                    raise RuntimeError("ASCII layout mapping unavailable: " + repr(char))
                if mapped & 0x100:
                    self._send(16)
                self._send(mapped & 0xff)
                self._send(mapped & 0xff, False)
                if mapped & 0x100:
                    self._send(16, False)
            time.sleep(0.012)
        self.record("type", text=text, method="Win32 scan codes / committed Unicode; not IME")

    def _pointer(self, x, y):
        self._foreground()
        point = w.POINT(x, y)
        self.user.ClientToScreen(self.hwnd, c.byref(point))
        if not self.user.LogicalToPhysicalPointForPerMonitorDPI(self.hwnd, c.byref(point)):
            raise c.WinError(c.get_last_error())
        self.user.SetPhysicalCursorPos(point.x, point.y)
        under = self.user.WindowFromPhysicalPoint(point)
        if self.user.GetAncestor(under, 2) != self.hwnd:
            raise RuntimeError("Refusing pointer injection outside owned browser")
        actual = w.POINT()
        self.user.GetPhysicalCursorPos(c.byref(actual))
        if (actual.x, actual.y) != (point.x, point.y):
            raise RuntimeError("Desktop pointer moved before owned-window injection")

    def click(self, x, y):
        self._pointer(x, y + self.page_origin[1])
        for flags in (2, 4):
            item = self.Input()
            item.kind = 0
            item.event.mouse = self.Mouse(0, 0, 0, flags, 0, 0)
            self._input(item)
            time.sleep(0.12)
        self.record("click", page=[x, y])

    def scroll(self, notches):
        self._pointer(400, 300 + self.page_origin[1])
        item = self.Input()
        item.kind = 0
        item.event.mouse = self.Mouse(0, 0, (notches * 120) & 0xffffffff, 0x0800, 0, 0)
        self._input(item)
        self.record("scroll", notches=notches)
        time.sleep(0.15)

    def title(self):
        text = c.create_unicode_buffer(8192)
        self.user.GetWindowTextW(self.hwnd, text, len(text))
        return text.value

    def url(self):
        caption = self.title()
        value = caption.rsplit(" — ", 1)[-1].rsplit(" [", 1)[0]
        return value if value.startswith(("http://", "https://", "file://", "demo://")) else None

    def tab_count(self):
        from PIL import Image
        path = self.output / ("observed-tabs-" + str(len(self.actions)) + ".png")
        self.screenshot(path)
        image = Image.open(path).convert("RGB")

        def cross(x, y):
            def dark(dx, dy):
                return sum(image.getpixel((x + dx, y + dy))) < 510
            return all(dark(a, b) for a, b in [(0, 0), (-2, -2), (2, 2), (-2, 2), (2, -2)]) and not dark(4, 0) and not dark(0, 4)

        found = []
        for count in range(1, 11):
            width = min(220.0, (image.width - 40.0) / count)
            if all(cross(int(6 + (i + 1) * width - 21) + 8, 18) for i in range(count)):
                found.append(count)
        actual = max(found, default=0)
        self.record("observed_tab_icons", count=actual, screenshot=str(path), sha256=digest(path))
        return actual

    def navigate(self, url):
        self.chord("Control", "l")
        self.type_text(str(url))
        self.key("Enter")
        self.record("navigate", url=str(url))

    def back(self): self.chord("Alt", "ArrowLeft")
    def forward(self): self.chord("Alt", "ArrowRight")
    def reload(self): self.chord("Alt", "r")
    def new_tab(self): self.chord("Control", "t")
    def select_tab(self, index): self.chord("Control", str(index + 1 if index < 8 else 9))
    def close_tab(self): self.chord("Control", "w")

    def screenshot(self, path):
        import io
        import struct
        from PIL import Image
        u, g = self.user, self.gdi
        u.GetClientRect.argtypes = [w.HWND, c.POINTER(w.RECT)]
        u.GetDC.argtypes, u.GetDC.restype = [w.HWND], w.HDC
        u.ReleaseDC.argtypes = [w.HWND, w.HDC]
        g.CreateCompatibleDC.argtypes, g.CreateCompatibleDC.restype = [w.HDC], w.HDC
        g.CreateCompatibleBitmap.argtypes, g.CreateCompatibleBitmap.restype = [w.HDC, c.c_int, c.c_int], w.HBITMAP
        g.SelectObject.argtypes, g.SelectObject.restype = [w.HDC, w.HANDLE], w.HANDLE
        u.PrintWindow.argtypes, u.PrintWindow.restype = [w.HWND, w.HDC, w.UINT], w.BOOL
        g.PatBlt.argtypes = [w.HDC, c.c_int, c.c_int, c.c_int, c.c_int, w.DWORD]
        g.GetDIBits.argtypes = [w.HDC, w.HBITMAP, w.UINT, w.UINT, c.c_void_p, c.c_void_p, w.UINT]
        g.DeleteObject.argtypes, g.DeleteDC.argtypes = [w.HANDLE], [w.HDC]
        rect = w.RECT()
        u.GetClientRect(self.hwnd, c.byref(rect))
        width, height = rect.right, rect.bottom
        dc = u.GetDC(self.hwnd)
        memory = g.CreateCompatibleDC(dc)
        bitmap = g.CreateCompatibleBitmap(dc, width, height)
        old = g.SelectObject(memory, bitmap)
        try:
            # Ask the exact HWND to draw its client into our empty bitmap.
            # Screen/DC blits can include unrelated desktop pixels under DPI
            # virtualization. Never fall back to a desktop capture here.
            g.PatBlt(memory, 0, 0, width, height, 0x00000042)
            if not u.PrintWindow(self.hwnd, memory, 1):  # PW_CLIENTONLY
                raise c.WinError(c.get_last_error())
            g.SelectObject(memory, old)
            header = struct.pack("<IiiHHIIiiII", 40, width, height, 1, 32, 0, width * height * 4, 0, 0, 0, 0)
            info, pixels = c.create_string_buffer(header + bytes(16)), c.create_string_buffer(width * height * 4)
            if g.GetDIBits(memory, bitmap, 0, height, pixels, info, 0) != height:
                raise c.WinError(c.get_last_error())
            raw = struct.pack("<2sIHHI", b"BM", 54 + len(pixels), 0, 0, 54) + header + pixels.raw
            Image.open(io.BytesIO(raw)).convert("RGB").save(path)
        finally:
            g.DeleteObject(bitmap)
            g.DeleteDC(memory)
            u.ReleaseDC(self.hwnd, dc)
        return str(path)

    def close(self):
        restore_error = None
        for vk in list(self.held):
            self._send(vk, False)
        if self.hwnd and self.user.IsWindow(self.hwnd):
            if self.original_layout:
                try:
                    self.set_layout(self.original_layout)
                    self.record("restore_layout", layout=hex(self.layout()))
                except Exception as error:
                    restore_error = error
                    self.record("restore_layout_failed", error=repr(error))
            self.user.PostMessageW(self.hwnd, 0x0010, 0, 0)
        if self.proc.poll() is None:
            try:
                self.proc.wait(timeout=8)
            except subprocess.TimeoutExpired:
                self.proc.terminate()
                self.proc.wait(timeout=8)
                self.record("forced_owned_child_cleanup")
        self.log.close()
        if self.original_dpi_context:
            self.user.SetThreadDpiAwarenessContext(self.original_dpi_context)
        (self.output / "actions.json").write_text(json.dumps(self.actions, indent=2) + "\n", encoding="utf-8")
        if restore_error:
            raise restore_error


class ChromeReference:
    mode = "reference-chrome-cdp"
    page_origin = (0, 0)
    capabilities = {"engine": "Chrome", "platform": "native Windows reference engine",
                    "real_window": False, "native_ime_composition": False,
                    "screenshot_includes_chrome": False, "screenshot_region": "CDP page 800x600",
                    "navigation_method": "CDP navigation/history plus actual Input events; isolated profile"}

    def __init__(self, binary, url, output):
        import websocket
        self.binary, self.output = Path(binary).resolve(), Path(output).resolve()
        self.output.mkdir(parents=True, exist_ok=True)
        profile = self.output / "fresh-profile"
        profile.mkdir(exist_ok=False)
        self.actions, self.serial, self.targets = [], 0, []
        self.log = (self.output / "reference.log").open("wb")
        self.proc = subprocess.Popen([str(self.binary), "--headless=new", "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1", "--user-data-dir=" + str(profile),
            "--no-first-run", "--disable-background-networking", "--disable-sync", "--disable-extensions",
            "--window-size=800,600", "about:blank"], stdout=self.log, stderr=subprocess.STDOUT)
        self.socket, self.browser_socket = None, None
        try:
            port_file = profile / "DevToolsActivePort"
            wait(port_file.is_file, "owned Chrome debug endpoint")
            self.port = int(port_file.read_text().splitlines()[0])
            self.ws_factory = lambda address: websocket.create_connection(address, timeout=8, suppress_origin=True)
            self.browser_socket = self.ws_factory("ws://127.0.0.1:" + str(self.port) + port_file.read_text().splitlines()[1])
            target = wait(lambda: next((x for x in self._target_list() if x["type"] == "page"), None), "reference page")
            self.targets.append(target["id"])
            self._attach(target["id"])
            self.version = self.browser_call("Browser.getVersion")
            self.capabilities = {**self.capabilities, "actual_browser_version": self.version}
            self.record("reference_launch", pid=self.proc.pid, binary_sha256=digest(self.binary),
                        actual_browser_version=self.version, isolated_profile=str(profile))
            self.navigate(url)
        except BaseException:
            self.close()
            raise

    def _target_list(self):
        with urllib.request.urlopen(f"http://127.0.0.1:{self.port}/json/list", timeout=3) as response:
            return json.load(response)

    def _attach(self, target):
        if self.socket:
            self.socket.close()
        page = next(x for x in self._target_list() if x["id"] == target)
        self.socket = self.ws_factory(page["webSocketDebuggerUrl"])
        self.active = target
        self.call("Page.enable")
        self.call("Runtime.enable")
        self.call("Emulation.setDeviceMetricsOverride", width=800, height=600, deviceScaleFactor=1, mobile=False)

    def call(self, method, **params):
        return self._call(self.socket, method, params)

    def browser_call(self, method, **params):
        return self._call(self.browser_socket, method, params)

    def _call(self, socket, method, params):
        self.serial += 1
        serial = self.serial
        socket.send(json.dumps({"id": serial, "method": method, "params": params}))
        while True:
            result = json.loads(socket.recv())
            if result.get("id") == serial:
                if "error" in result:
                    raise RuntimeError((method, result["error"]))
                return result.get("result", {})

    def evaluate(self, expression):
        result = self.call("Runtime.evaluate", expression=expression, returnByValue=True)
        if "exceptionDetails" in result:
            raise RuntimeError(result["exceptionDetails"])
        return result.get("result", {}).get("value")

    def record(self, action, **data):
        self.actions.append({"action": action, "monotonic": time.monotonic(), **data})

    def navigate(self, url):
        result = self.call("Page.navigate", url=str(url))
        self.record("navigate", url=str(url), result=result)
        if not result.get("errorText"):
            wait(lambda: self.evaluate("document.readyState") == "complete", "reference document")

    def title(self): return self.evaluate("document.title")
    def url(self): return self.evaluate("location.href")
    def tab_count(self): return len([p for p in self._target_list() if p["type"] == "page"])

    def click(self, x, y):
        self.call("Input.dispatchMouseEvent", type="mousePressed", x=x, y=y, button="left", clickCount=1)
        self.call("Input.dispatchMouseEvent", type="mouseReleased", x=x, y=y, button="left", clickCount=1)
        self.record("click", page=[x, y])

    def type_text(self, text):
        self.call("Input.insertText", text=text)
        self.record("type", text=text, method="CDP committed text; not OS IME")

    def key(self, name):
        vk = KEYS.get(name, ord(name.upper()) if len(name) == 1 else None)
        for kind in ("keyDown", "keyUp"):
            params = dict(type=kind, key=" " if name == "Space" else name, windowsVirtualKeyCode=vk or 0)
            if name == "Enter" and kind == "keyDown":
                params["text"] = "\r"
            self.call("Input.dispatchKeyEvent", **params)
        self.record("key", key=name)

    def scroll(self, notches):
        self.call("Input.dispatchMouseEvent", type="mouseWheel", x=400, y=300, deltaX=0, deltaY=-notches * 120)
        self.record("scroll", notches=notches)

    def _history(self, offset):
        state = self.call("Page.getNavigationHistory")
        index = state["currentIndex"] + offset
        self.call("Page.navigateToHistoryEntry", entryId=state["entries"][index]["id"])
        time.sleep(0.12)

    def back(self): self._history(-1)
    def forward(self): self._history(1)
    def reload(self): self.call("Page.reload", ignoreCache=True)

    def new_tab(self):
        target = self.browser_call("Target.createTarget", url="about:blank")["targetId"]
        self.targets.append(target)
        self._attach(target)

    def select_tab(self, index): self._attach(self.targets[index])

    def close_tab(self):
        closing = self.active
        index = self.targets.index(closing)
        self.browser_call("Target.closeTarget", targetId=closing)
        self.targets.remove(closing)
        if self.targets:
            self._attach(self.targets[min(index, len(self.targets) - 1)])

    def screenshot(self, path):
        import base64
        Path(path).write_bytes(base64.b64decode(self.call("Page.captureScreenshot", format="png")["data"]))
        return str(path)

    def close(self):
        if self.browser_socket:
            try:
                self.browser_call("Browser.close")
            except Exception:
                pass
            self.browser_socket.close()
        if self.socket:
            self.socket.close()
        if self.proc.poll() is None:
            try:
                self.proc.wait(timeout=8)
            except subprocess.TimeoutExpired:
                self.proc.terminate()
                self.proc.wait(timeout=8)
        self.log.close()
        (self.output / "actions.json").write_text(json.dumps(self.actions, indent=2) + "\n", encoding="utf-8")
