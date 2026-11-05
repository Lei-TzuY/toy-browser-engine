"""Own-display X11 input adapter; no user's desktop or existing browser session."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import time


def wait(predicate, description, timeout=8):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        value = predicate()
        if value:
            return value
        time.sleep(0.05)
    raise RuntimeError("Timed out: " + description)


class X11Session:
    def __init__(self, output, prefix=None, font=None, fallback_font=None):
        self.output = Path(output).resolve()
        self.output.mkdir(parents=True, exist_ok=True)
        self.env = os.environ.copy()
        self.env.pop("WAYLAND_DISPLAY", None)
        self.prefix = Path(prefix).resolve() if prefix else None
        if self.prefix:
            self.env["PATH"] = str(self.prefix / "usr/bin") + os.pathsep + self.env.get("PATH", "")
            lib = self.prefix / "usr/lib/x86_64-linux-gnu"
            self.env["LD_LIBRARY_PATH"] = str(lib) + os.pathsep + self.env.get("LD_LIBRARY_PATH", "")
            self.env["MAGICK_CONFIGURE_PATH"] = str(self.prefix / "etc/ImageMagick-7")
            self.env["MAGICK_CODER_MODULE_PATH"] = str(lib / "ImageMagick-7.1.2/modules-Q16/coders")
        if font:
            self.env["BROWSER_ENGINE_FONT"] = str(Path(font).resolve())
        if fallback_font:
            self.env["BROWSER_ENGINE_FALLBACK_FONT"] = str(Path(fallback_font).resolve())
        self.tools = {name: shutil.which(name, path=self.env["PATH"]) for name in ("Xvfb", "xdotool", "import")}
        if not all(self.tools.values()):
            raise RuntimeError("Missing user-prefix/system X11 tools: " + repr(self.tools))
        self.log = (self.output / "xvfb.log").open("wb")
        self.proc = None
        for number in range(280, 380):
            if Path("/tmp/.X%d-lock" % number).exists():
                continue
            self.env["DISPLAY"] = ":%d" % number
            self.proc = subprocess.Popen([self.tools["Xvfb"], self.env["DISPLAY"], "-screen", "0", "1024x768x24", "-nolisten", "tcp", "-ac", "-noreset"], env=self.env, stdout=self.log, stderr=subprocess.STDOUT)
            time.sleep(0.4)
            if self.proc.poll() is None:
                probe = self.xdo("getdisplaygeometry", check=False)
                if probe:
                    break
                self.proc.terminate()
                self.proc.wait(timeout=4)
        else:
            self.close()
            raise RuntimeError("Could not create an isolated Xvfb display")

    def xdo(self, *args, check=True):
        proc = subprocess.run([self.tools["xdotool"], *[str(arg) for arg in args]], env=self.env, capture_output=True, text=True)
        if check and proc.returncode:
            raise RuntimeError("xdotool " + repr(args) + ": " + proc.stderr)
        return proc.stdout.strip()

    def close(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            self.proc.wait(timeout=5)
        self.log.close()


class X11Toy:
    mode = "linux-x11-gui"
    page_origin = (0, 68)
    capabilities = {"engine": "toy-browser", "platform": "WSL Linux X11",
                    "real_window": True, "native_ime_composition": False,
                    "screenshot_includes_chrome": True, "screenshot_region": "owned X11 client",
                    "navigation_method": "XTest keyboard/pointer on isolated owned Xvfb display",
                    "window_ownership": "one owned browser child per private display; minifb does not set _NET_WM_PID"}

    def __init__(self, session, binary, url, output):
        self.session, self.binary, self.output = session, Path(binary).resolve(), Path(output).resolve()
        self.output.mkdir(parents=True, exist_ok=True)
        self.actions, self.held, self.wid = [], set(), None
        self.log = (self.output / "browser.log").open("wb")
        self.proc = subprocess.Popen([str(self.binary), "--window", "--quiet", "--size", "800x600", str(url)], env=session.env, cwd=self.output, stdout=self.log, stderr=subprocess.STDOUT)
        try:
            def find():
                if self.proc.poll() is not None:
                    raise RuntimeError("Owned browser child exited; see browser.log")
                found = self.session.xdo("search", "--onlyvisible", "--name", ".", check=False).splitlines()
                return found[0] if len(found) == 1 else None
            self.wid = wait(find, "single browser window on owned display")
            self.session.xdo("windowfocus", self.wid)
            self._focus()
            # Window title/visibility can precede minifb's first buffer. Wait
            # for pixels from the actual owned client, including slow font
            # parsing, rather than treating an unpainted black window as a page.
            from PIL import Image
            startup = self.output / "first-painted-frame.png"
            def painted():
                self.screenshot(startup)
                image = Image.open(startup).convert("RGB")
                return image.size == (800, 668) and image.getpixel((799, 0)) == (222, 225, 230)
            wait(painted, "owned client first painted frame", timeout=12)
            self.record("launch", pid=self.proc.pid, window=self.wid, display=self.session.env["DISPLAY"],
                        font=self.session.env.get("BROWSER_ENGINE_FONT"),
                        fallback_font=self.session.env.get("BROWSER_ENGINE_FALLBACK_FONT"))
        except BaseException:
            self.close()
            raise

    def record(self, action, **details):
        self.actions.append({"action": action, "monotonic": time.monotonic(), **details})

    def _focus(self):
        if self.proc.poll() is not None:
            raise RuntimeError("Owned browser child exited")
        if self.session.xdo("getwindowfocus") != self.wid:
            raise RuntimeError("Refusing X11 input outside owned browser window")

    def _chord(self, value):
        self._focus()
        try:
            self.session.xdo("keydown", value)
            self.held.add(value)
            time.sleep(0.12)
        finally:
            self.session.xdo("keyup", value)
            self.held.discard(value)
        self.record("key chord", value=value)
        time.sleep(0.12)

    def key(self, name):
        value = {"Enter": "Return", "Backspace": "BackSpace", "ArrowDown": "Down", "Space": "space"}.get(name, name)
        self._chord(value)

    def type_text(self, text):
        self._focus()
        if not text.isascii():
            raise RuntimeError("X11 adapter only certifies ASCII typing; Unicode committed input is separate")
        self.session.xdo("type", "--delay", "40", "--", text)
        self.record("type", text=text, method="xdotool XTest text input; no IME")
        time.sleep(0.2)

    def _pointer(self, x, y):
        self._focus()
        if not (0 <= x < 800 and 0 <= y < 668):
            raise RuntimeError("Refusing pointer injection outside owned client geometry")
        self.session.xdo("mousemove", "--window", self.wid, x, y)
        location = self.session.xdo("getmouselocation", "--shell")
        fields = dict(line.split("=", 1) for line in location.splitlines() if "=" in line)
        if fields.get("WINDOW") != self.wid:
            raise RuntimeError("Refusing pointer input over another X11 window: " + repr(fields))

    def _click_client(self, x, y):
        self._pointer(x, y)
        self.session.xdo("mousedown", "1")
        time.sleep(0.12)
        self.session.xdo("mouseup", "1")
        self.record("click", client=[x, y])
        time.sleep(0.25)

    def click(self, x, y):
        self._click_client(x, y + self.page_origin[1])

    def scroll(self, notches):
        self._pointer(400, 368)
        for unused in range(abs(notches)):
            self.session.xdo("click", "5" if notches < 0 else "4")
            time.sleep(0.05)
        self.record("scroll", notches=notches)
        time.sleep(0.25)

    def navigate(self, url):
        self._chord("ctrl+l")
        self.type_text(str(url))
        self.key("Enter")
        self.record("navigate", url=str(url))

    def back(self): self._click_client(20, 50)
    def forward(self): self._click_client(52, 50)
    def reload(self): self._click_client(84, 50)
    def new_tab(self): self._chord("ctrl+t")
    def select_tab(self, index): self._chord("ctrl+" + str(index + 1 if index < 8 else 9))
    def close_tab(self): self._chord("ctrl+w")

    def title(self):
        return self.session.xdo("getwindowname", self.wid)

    def url(self):
        title = self.title()
        return title.rsplit(" — ", 1)[-1].rsplit(" [", 1)[0] if " — " in title else None

    def screenshot(self, path):
        subprocess.run([self.session.tools["import"], "-window", self.wid, str(path)], env=self.session.env, check=True, capture_output=True)
        return str(path)

    def tab_count(self):
        from PIL import Image
        path = self.output / "observed-tab-strip.png"
        self.screenshot(path)
        image = Image.open(path).convert("RGB")
        def dark(x, y): return sum(image.getpixel((int(x), int(y)))) < 510
        def cross(x, y):
            return all(dark(x + dx, y + dy) for dx, dy in [(0, 0), (-2, -2), (2, 2), (-2, 2), (2, -2)]) and not dark(x + 4, y) and not dark(x, y + 4)
        found = []
        for number in range(1, 11):
            width = min(220.0, (800 - 12 - 4 - 24) / number)
            if all(cross(int(6 + (index + 1) * width - 5 - 16) + 8, 18) for index in range(number)):
                found.append(number)
        # Capped-width tabs share their early close-icon positions. A two-tab
        # strip therefore also matches the one-tab prefix; the largest full
        # match counts all observed close icons, as in the existing GUI test.
        return max(found) if found else None

    def close(self):
        for value in list(self.held):
            self.session.xdo("keyup", value, check=False)
        self.held.clear()
        self.session.xdo("keyup", "ctrl", "shift", "alt", "Return", "space", check=False)
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=3)
        self.log.close()
        (self.output / "actions.json").write_text(json.dumps(self.actions, indent=2) + "\n", encoding="utf-8")
