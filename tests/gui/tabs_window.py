#!/usr/bin/env python3
"""Drive `browser_engine --window` under Xvfb with xdotool and check tabs.

Every check reads the real window: its title (which shows the active tab's
page title, URL, history position and any error) or a screenshot of it.

Not part of `cargo test`: it needs a real X server, xdotool, ImageMagick's
`import` and Pillow. Use a release build — a debug build paints slowly enough
that minifb, which samples input once per frame, can miss short key taps.

    cargo build --release
    Xvfb :99 -screen 0 1024x768x24 &
    DISPLAY=:99 python3 tests/gui/tabs_window.py \
        target/release/browser_engine /tmp/tabs-gui

Screenshots, the served test pages and the browser's log land in the output
directory. Exits non-zero if any check fails.
"""
import http.server
import os
import socketserver
import subprocess
import sys
import threading
import time

from PIL import Image, ImageChops

BINARY, OUT = sys.argv[1], sys.argv[2]
os.makedirs(OUT, exist_ok=True)
SITE = os.path.abspath(os.path.join(OUT, "site"))
os.makedirs(SITE, exist_ok=True)

STRIP_H, BAR_H, W, H = 32, 36, 800, 600
PAGE_TOP = STRIP_H + BAR_H

PAGES = {
    "one.html": """<html><head><title>One</title></head><body style="margin:0">
<a href="link-target.html" style="display:block;height:150px;background-color:#cc3333;color:white">TOP LINK</a>
<div style="height:400px;background-color:#3333aaa33">green</div>
<div style="height:400px;background-color:#3333aa">blue</div>
<div style="height:2000px;background-color:#aaaa33">yellow</div>
</body></html>""",
    "two.html": """<html><head><title>Two</title></head><body style="margin:0">
<a href="three.html" style="display:block;height:100px;background-color:#3366cc;color:white">to three</a>
</body></html>""",
    "three.html": "<html><head><title>Three</title></head><body><p>three</p></body></html>",
    "link-target.html": "<html><head><title>LinkTarget</title></head><body><p>target</p></body></html>",
    "keys.html": """<html><head><title>Keys</title></head><body><p id="log">none</p>
<script>
  let keys = "";
  document.body.addEventListener("keydown", function (event) {
    keys = keys + event.key;
    document.querySelector("title").textContent = "Keys " + keys;
  });
</script></body></html>""",
}
for name, html in PAGES.items():
    with open(os.path.join(SITE, name), "w") as f:
        f.write(html)


class Quiet(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **k):
        super().__init__(*a, directory=SITE, **k)

    def log_message(self, *a):
        pass


server = socketserver.TCPServer(("127.0.0.1", 0), Quiet)
PORT = server.server_address[1]
threading.Thread(target=server.serve_forever, daemon=True).start()
BASE = f"http://127.0.0.1:{PORT}"

results = []


def check(name, ok, detail=""):
    results.append((name, ok, detail))
    print(("PASS " if ok else "FAIL ") + name + (f"  [{detail}]" if detail else ""), flush=True)


def xdo(*args):
    return subprocess.run(["xdotool", *args], capture_output=True, text=True).stdout.strip()


log = open(os.path.join(OUT, "browser.log"), "w")
proc = subprocess.Popen(
    [BINARY, "--window", "--quiet", f"{BASE}/one.html"], stdout=log, stderr=subprocess.STDOUT
)

wid = ""
for _ in range(100):
    wid = xdo("search", "--name", "One")
    if wid:
        break
    time.sleep(0.1)
wid = wid.splitlines()[0]
geometry = xdo("getwindowgeometry", wid)
check("window opens with tab strip + toolbar above an 800x600 page", "800x668" in geometry, geometry.replace("\n", " "))
xdo("windowfocus", wid)
xdo("mousemove", "--window", wid, "400", "400")
time.sleep(0.5)


def title():
    return xdo("getwindowname", wid)


def wait_title(predicate, timeout=6.0):
    end = time.time() + timeout
    t = title()
    while time.time() < end:
        t = title()
        if predicate(t):
            return True, t
        time.sleep(0.05)
    return False, t


def expect(name, prefix, contains=None, timeout=6.0):
    ok, t = wait_title(
        lambda t: t.startswith(prefix) and (contains is None or contains in t), timeout
    )
    check(name, ok, t)
    return ok


def settle():
    time.sleep(0.35)


# minifb samples key and button state once per frame, so a press and release
# inside one 16 ms frame can be missed. Hold each one for a few frames, as a
# person would.
HOLD = 0.12


def key(*keys):
    for combo in keys:
        xdo("keydown", combo)
        time.sleep(HOLD)
        xdo("keyup", combo)
        time.sleep(0.05)
    settle()


def type_text(text):
    xdo("type", "--delay", "40", text)
    settle()


def click(x, y, button="1"):
    xdo("mousemove", "--window", wid, str(x), str(y))
    time.sleep(0.1)
    xdo("mousedown", button)
    time.sleep(HOLD)
    xdo("mouseup", button)
    settle()


def scroll(down, clicks):
    xdo("mousemove", "--window", wid, "400", "400")
    for _ in range(clicks):
        xdo("click", "5" if down else "4")
        time.sleep(0.05)
    settle()


def shot(name):
    path = os.path.join(OUT, name + ".png")
    subprocess.run(["import", "-window", wid, path], check=True)
    return Image.open(path).convert("RGB")


def page(img):
    return img.crop((0, PAGE_TOP, W, PAGE_TOP + H))


def same(a, b):
    return ImageChops.difference(a, b).getbbox() is None


def tab_width(n):
    return min(220.0, (W - 12 - 4 - 24) / n)


def tab_center(i, n):
    return int(6 + (i + 0.5) * tab_width(n)), 18


def close_center(i, n):
    # The close button's box is 16px wide, so its centre pixel is at +8.
    return int(6 + (i + 1) * tab_width(n) - 5 - 16) + 8, 18


def new_tab_center(n):
    return int(6 + n * tab_width(n) + 4 + 12), 18


BACK, FORWARD = (20, STRIP_H + 18), (52, STRIP_H + 18)
TOOLBAR_BG = (241, 243, 244)


def dark(img, x, y):
    return sum(img.getpixel((int(x), int(y)))) < 3 * 170


def cross_at(img, cx, cy):
    """A tab close button's "x" is painted centred on (cx, cy)."""
    diagonal = all(dark(img, cx + dx, cy + dy) for dx, dy in [(0, 0), (-2, -2), (2, 2), (-2, 2), (2, -2)])
    return diagonal and not dark(img, cx + 4, cy) and not dark(img, cx, cy + 4)


def tab_count(img):
    """Tabs drawn in the strip: the count whose close buttons are all there."""
    found = [
        n for n in range(1, 11) if all(cross_at(img, *close_center(i, n)) for i in range(n))
    ]
    # Up to three tabs are all full width, so their close buttons are a
    # prefix of a bigger strip's: the most tabs that match is the count.
    return max(found, default=0)


def highlighted(img, n):
    """Indices of tabs drawn in the toolbar colour (the active one)."""
    return [i for i in range(n) if img.getpixel((int(6 + i * tab_width(n) + 4), STRIP_H - 1)) == TOOLBAR_BG]


STRIP_SHOTS = [0]


def strip_is(name, count, active):
    STRIP_SHOTS[0] += 1
    img = shot(f"strip-{STRIP_SHOTS[0]:02d}")
    got = (tab_count(img), highlighted(img, count))
    check(name, got == (count, [active]), f"tabs={got[0]} active={got[1]}")

# ── 1. One tab, scrolled ─────────────────────────────────────────────────────
expect("starts on One [1/1]", "One", "[1/1]")
top = page(shot("01-one-top"))
scroll(True, 5)
scrolled = page(shot("02-one-scrolled"))
check("wheel scrolls tab 1's page", not same(top, scrolled))

# ── 2. Ctrl+T, then navigate the new tab from its focused address ─────────────
key("ctrl+t")
expect("Ctrl+T opens the home page in a new tab", "One", "[1/1]")
strip_is("strip shows 2 tabs, the new one active", 2, 1)
type_text(f"{BASE}/two.html")
key("Return")
expect("typing into the new tab's focused address navigates it", "Two", "[2/2]")
shot("03-two-tabs")

# ── 3. Links and history inside tab 2 ────────────────────────────────────────
click(400, PAGE_TOP + 50)
expect("link click in tab 2 navigates tab 2", "Three", "[3/3]")
click(*BACK)
expect("Back button goes back in tab 2", "Two", "[2/3]")

# ── 4. Ctrl+Tab back to tab 1: its scroll and history are intact ─────────────
key("ctrl+Tab")
expect("Ctrl+Tab switches to tab 1", "One", "[1/1]")
strip_is("strip highlights tab 1", 2, 0)
check("tab 1 kept its scroll position", same(page(shot("04-back-to-one")), scrolled))
click(*BACK)
expect("Back is disabled in tab 1 (tab 2's history not shared)", "One", "[1/1]", timeout=1.0)

# ── 5. Ctrl+Shift+Tab, Forward/Back act on tab 2 only ────────────────────────
key("ctrl+shift+Tab")
expect("Ctrl+Shift+Tab switches back to tab 2", "Two", "[2/3]")
click(*FORWARD)
expect("Forward in tab 2", "Three", "[3/3]")
click(*BACK)
expect("Back in tab 2 again", "Two", "[2/3]")

# ── 6. Unsent address text survives a round trip through another tab ─────────
key("ctrl+l")
type_text(f"{BASE}/three.html")
key("ctrl+Tab")
expect("switch away while editing: tab 1 shows its own state", "One", "[1/1]")
type_text("abc")  # tab 1's page; must not reach tab 2's address field
key("ctrl+Tab")
key("Return")
expect("draft address preserved and submitted in tab 2", "Three", "[3/3]")

# ── 7. Keyboard isolation for page key listeners ─────────────────────────────
key("ctrl+t")
type_text(f"{BASE}/keys.html")
key("Return")
expect("third tab on the key logger", "Keys")
type_text("ab")
expect("page receives ordinary keys", "Keys ab")
key("ctrl+Tab")
key("ctrl+shift+Tab")
expect("Ctrl+Tab / Ctrl+Shift+Tab never reach the page", "Keys ab —")
key("ctrl+t")
key("ctrl+w")
expect("Ctrl+T then Ctrl+W leave the key logger untouched", "Keys ab —")
key("ctrl+l")
type_text("zz")
key("Escape")
expect("address typing never reaches the page", "Keys ab —")
type_text("c")
expect("page keys resume after Escape", "Keys abc —")

# ── 8. Two key-logger tabs keep separate document state ──────────────────────
key("ctrl+t")
type_text(f"{BASE}/keys.html")
key("Return")
type_text("z")
expect("fourth tab logs only its own keys", "Keys z —")
key("ctrl+3")
expect("Ctrl+3 selects the third tab with its own log", "Keys abc —")
key("ctrl+1")
expect("Ctrl+1 selects the first tab", "One", "[1/1]")
key("ctrl+9")
expect("Ctrl+9 selects the last tab", "Keys z —")
shot("05-four-tabs")
strip_is("strip shows 4 tabs, the last active", 4, 3)

# ── 8b. A file:// page in its own tab ───────────────────────────────────────
key("ctrl+t")
type_text("file://" + os.path.join(SITE, "three.html"))
key("Return")
expect("a new tab can open a local file:// page", "Three — file://")
key("ctrl+1")
expect("switching from the file tab to an HTTP tab", "One", "[1/1]")
key("ctrl+9")
expect("and back to the file tab", "Three — file://")
key("ctrl+w")
expect("closing the last-position tab activates its left neighbour", "Keys z —")
strip_is("back to 4 tabs", 4, 3)

# ── 9. A failed navigation keeps that tab's page; other tabs unaffected ──────
key("ctrl+l")
type_text("http://127.0.0.1:9/nothing")
key("Return")
expect("failed navigation reports an error and keeps the page", "Could not open http://127.0.0.1:9/nothing", "Keys z")
shot("06-navigation-error")
key("ctrl+1")
ok, t = wait_title(lambda t: t.startswith("One"))
check("other tabs show no error", ok and "Could not" not in t, t)
key("ctrl+9")
expect("the failed tab still shows its error on return", "Could not open", "Keys z")
key("Escape")
expect("Escape clears the failed edit", "Keys z —")

# ── 10. Close button, middle click, "+" button ───────────────────────────────
click(*close_center(1, 4))
expect("close button closes tab 2; active tab unchanged", "Keys z —")
strip_is("3 tabs after the close button", 3, 2)
click(*tab_center(1, 3), button="2")
expect("middle click closes the clicked tab", "Keys z —")
strip_is("2 tabs after the middle click", 2, 1)
click(*tab_center(0, 2))
expect("clicking a tab selects it", "One", "[1/1]")
click(*tab_center(1, 2))
expect("clicking the other tab selects it", "Keys z —")
click(*new_tab_center(2))
expect("the + button opens a new tab", "One", "[1/1]")
strip_is("3 tabs after +, the new one active", 3, 2)
key("Escape")
shot("07-three-tabs")

# ── 11. Clicks on the strip never reach the page underneath ─────────────────
key("ctrl+1")
scroll(False, 10)
for _ in range(3):
    click(*tab_center(0, 3))  # above One's full-width top link
ok, t = wait_title(lambda t: "LinkTarget" in t, timeout=1.0)
check("clicking a tab never activates the page link below it", not ok and t.startswith("One"), t)
click(400, PAGE_TOP + 75)
expect("the same link works when clicked in the page", "LinkTarget", "[2/2]")
click(*BACK)
expect("Back returns to One", "One", "[1/2]")

# ── 12. Closing the last tab closes the window ──────────────────────────────
key("ctrl+w")
expect("Ctrl+W closes the active tab; its right neighbour takes over", "Keys z —")
key("ctrl+w")
expect("closing tabs one by one", "One", "[1/1]")
strip_is("one tab left", 1, 0)
key("ctrl+w")
try:
    code = proc.wait(timeout=5)
except subprocess.TimeoutExpired:
    code = None
    proc.kill()
check("Ctrl+W on the last tab closes the window and exits cleanly", code == 0, f"exit {code}")

log.close()
server.shutdown()
failed = [r for r in results if not r[1]]
print(f"\n{len(results) - len(failed)}/{len(results)} GUI checks passed")
sys.exit(1 if failed else 0)
