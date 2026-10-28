#!/usr/bin/env python3
"""Real-window R1/R2 probes; run with a release binary and an output directory.

Starts an isolated TCP-only Xvfb (also works where AF_UNIX is unavailable).
Requires Xvfb and xdotool. Each slow-load trial waits for the server to enter
its 1.5 s delay, types during that delay, then submits the exact editor contents
to a local capture URL. No DNS, OCR, clipboard or product instrumentation.

    python3 tests/gui/shortcut_routing.py target/release/browser_engine /tmp/routing
    # The original binary is expected to FAIL the R1 and staggered R2 probes.
"""
import argparse
import contextlib
import ctypes
import http.server
import json
import os
from pathlib import Path
import queue
import signal
import socket
import subprocess
import threading
import time
import urllib.parse


PAGE = b'''<html><head><title>RoutingKeys</title></head><body><p>keys</p>
<script>let keys = "";
document.body.addEventListener("keydown", function(e) {
  keys = keys + e.key;
  document.querySelector("title").textContent = "RoutingKeys " + keys;
});</script></body></html>'''


class Harness:
    def __init__(self, binary, out, trials):
        self.binary, self.out, self.trials = str(Path(binary).resolve()), out, trials
        self.results = []
        self.captures = queue.Queue()
        self.slow_started, self.slow_finished = threading.Event(), threading.Event()
        self.slow_count = 0
        harness = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                if self.path.startswith('/slow'):
                    harness.slow_count += 1
                    if harness.slow_count > 1:
                        harness.slow_started.set()
                        time.sleep(1.5)
                        harness.slow_finished.set()
                if self.path.startswith('/capture/'):
                    harness.captures.put(urllib.parse.unquote(self.path[len('/capture/'):]))
                self.send_response(200)
                self.send_header('Content-Length', str(len(PAGE)))
                self.end_headers()
                self.wfile.write(PAGE)

            def log_message(self, *args):
                pass

        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.base = 'http://127.0.0.1:' + str(self.server.server_port)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.env = os.environ.copy()
        self.env.pop('WAYLAND_DISPLAY', None)
        # Xvfb displays use port 6000 + display number. Test availability first.
        for number in range(110, 210):
            with socket.socket() as sock:
                try:
                    sock.bind(('127.0.0.1', 6000 + number))
                except OSError:
                    continue
            self.env['DISPLAY'] = f'127.0.0.1:{number}'
            self.xlog = open(out / 'xvfb.log', 'w')
            self.xvfb = subprocess.Popen([
                'Xvfb', f':{number}', '-screen', '0', '1024x768x24',
                '-nolisten', 'local', '-nolisten', 'unix', '-listen', 'tcp', '-ac', '-noreset',
            ], stdout=self.xlog, stderr=subprocess.STDOUT)
            time.sleep(0.4)
            if self.xvfb.poll() is None:
                break
        else:
            raise RuntimeError('No Xvfb display available')

    def xdo(self, *args):
        return subprocess.run(['xdotool', *args], env=self.env, check=True,
                              capture_output=True, text=True).stdout.strip()

    def key(self, key):
        self.xdo('keydown', key)
        time.sleep(0.12)
        self.xdo('keyup', key)
        time.sleep(0.1)

    def text(self, text):
        self.xdo('type', '--delay', '25', '--', text)

    def title(self):
        return self.xdo('getwindowname', self.wid)

    @contextlib.contextmanager
    def window(self, slow=False):
        self.slow_count = 0
        self.slow_started.clear()
        self.slow_finished.clear()
        while not self.captures.empty():
            self.captures.get_nowait()
        log = open(self.out / 'browser.log', 'a')
        proc = subprocess.Popen([self.binary, '--window', '--quiet',
                                 self.base + ('/slow' if slow else '/keys')],
                                env=self.env, cwd=self.out, stdout=log,
                                stderr=subprocess.STDOUT)
        self.process = proc
        try:
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                found = subprocess.run(['xdotool', 'search', '--name', '^RoutingKeys'],
                                       env=self.env, capture_output=True, text=True).stdout
                if found:
                    self.wid = found.splitlines()[0]
                    break
                if proc.poll() is not None:
                    raise RuntimeError('Browser exited; see browser.log')
                time.sleep(0.05)
            else:
                raise RuntimeError('Browser window did not appear')
            self.xdo('windowfocus', self.wid)
            time.sleep(0.2)
            yield
        finally:
            # Release any keys even after a failed assertion.
            self.xdo('keyup', 'ctrl', 'shift', 'alt', 'Tab', 't', 'w', *list('123456789'))
            proc.terminate()
            proc.wait(timeout=5)
            log.close()

    def capture_editor(self):
        # Home preserves the existing draft. Prepend a local endpoint so even
        # www.example.com / localhost:8080 are observed without external I/O.
        self.key('Home')
        self.text(self.base + '/capture/')
        self.key('Return')
        return self.captures.get(timeout=5)

    def select(self, index):
        self.xdo('mousemove', '--window', self.wid, str(100 + index * 220), '18')
        self.xdo('mousedown', '1')
        time.sleep(0.1)
        self.xdo('mouseup', '1')
        time.sleep(0.12)

    def titles(self):
        values = []
        for index in range(2):
            self.select(index)
            values.append(self.title())
        return values

    def two_tabs(self, editing=False):
        self.key('ctrl+t')
        self.key('Escape')
        if editing:
            for index in range(2):
                self.select(index)
                self.key('ctrl+l')
                self.text('seed')

    def record(self, case, failures, trials, details):
        result = dict(case=case, failures=failures, trials=trials, details=details)
        self.results.append(result)
        print(json.dumps(result), flush=True)

    def slow_load(self):
        for text in ['aw1b', 'www.example.com', '127.0.0.1', 'localhost:8080']:
            details = []
            for _ in range(self.trials):
                with self.window(slow=True):
                    self.xdo('keydown', 'ctrl+t')
                    if not self.slow_started.wait(5):
                        raise RuntimeError('Ctrl+T did not start slow navigation')
                    self.xdo('keyup', 'ctrl+t')
                    self.text(text)
                    queued_during_load = not self.slow_finished.is_set()
                    if not self.slow_finished.wait(5):
                        raise RuntimeError('Slow navigation did not finish')
                    time.sleep(0.35)
                    actual = self.capture_editor()
                    details.append(dict(expected=text, actual=actual,
                                        queued_during_load=queued_during_load))
            failures = sum(d['actual'] != text or not d['queued_during_load'] for d in details)
            self.record('slow Ctrl+T: ' + text, failures, len(details), details)

    def staggered(self):
        details = []
        with self.window():
            self.two_tabs()
            for _ in range(self.trials):
                before = self.titles()
                self.xdo('keydown', 'ctrl+shift+Tab')
                time.sleep(0.7)
                self.xdo('keyup', 'ctrl')
                # Tab outlives Ctrl by many frames, including minifb repeats.
                time.sleep(0.35)
                self.xdo('keyup', 'shift')
                time.sleep(0.35)
                self.xdo('keyup', 'Tab')
                time.sleep(0.12)
                after = self.titles()
                details.append(dict(before=before, after=after))
        self.record('staggered Ctrl+Shift+Tab', sum(d['before'] != d['after'] for d in details),
                    len(details), details)

    def simultaneous(self, editing):
        details = []
        with self.window():
            self.two_tabs(editing)
            for key in ['Tab'] + list('123456789'):
                for _ in range(self.trials):
                    before = self.titles()
                    self.xdo('keydown', 'ctrl+' + key)
                    time.sleep(0.65)
                    self.xdo('keyup', 'ctrl+' + key)
                    time.sleep(0.1)
                    after = self.titles()
                    details.append(dict(key=key, before=before, after=after))
            drafts = []
            if editing:
                for index in range(2):
                    self.select(index)
                    drafts.append(self.capture_editor())
        failures = sum(d['before'] != d['after'] for d in details)
        if editing:
            failures += sum(d != 'seed' for d in drafts)
        self.record('held Ctrl+Tab / 1-9: ' + ('editor' if editing else 'DOM'), failures,
                    len(details) + len(drafts), dict(trials=details, drafts=drafts))

    def ordinary_after_release(self):
        details = []
        for _ in range(self.trials):
            with self.window():
                self.two_tabs()
                self.xdo('keydown', 'ctrl+Tab')
                time.sleep(0.15)
                self.xdo('keyup', 'ctrl+Tab')
                self.text('tw123456789')
                self.key('Tab')
                time.sleep(0.15)
                names = self.titles()
                details.append(names)
        self.record('ordinary input immediately after release',
                    sum(not any('RoutingKeys tw123456789Tab' in t for t in d) for d in details),
                    len(details), details)

    def unicode_repeats(self):
        # Unmapped X11 Unicode key: text arrives without set_key_state.
        x = ctypes.CDLL('libX11.so.6')
        xt = ctypes.CDLL('libXtst.so.6')
        x.XOpenDisplay.argtypes = [ctypes.c_char_p]
        x.XOpenDisplay.restype = ctypes.c_void_p
        x.XChangeKeyboardMapping.argtypes = [ctypes.c_void_p, ctypes.c_int,
                                            ctypes.c_int,
                                            ctypes.POINTER(ctypes.c_ulong), ctypes.c_int]
        x.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
        x.XCloseDisplay.argtypes = [ctypes.c_void_p]
        xt.XTestFakeKeyEvent.argtypes = [ctypes.c_void_p, ctypes.c_uint,
                                       ctypes.c_int, ctypes.c_ulong]
        display = x.XOpenDisplay(self.env['DISPLAY'].encode())
        if not display:
            raise RuntimeError('Unicode probe cannot open its X display')
        details = []
        try:
            x.XChangeKeyboardMapping(display, 200, 1,
                                    (ctypes.c_ulong * 1)(0x01005b57), 1)
            x.XSync(display, 0)
            for owned in [False, True]:
                for _ in range(self.trials):
                    with self.window():
                        if owned:
                            self.xdo('keydown', 'ctrl+1')
                            time.sleep(0.15)
                            self.xdo('keyup', 'ctrl')
                            time.sleep(0.12)
                        os.kill(self.process.pid, signal.SIGSTOP)
                        try:
                            # Real server repeats queue while the browser is
                            # paused. A duplicate XTest down alone is NOT a
                            # reliable repeat reproduction.
                            time.sleep(0.75)
                            for code, down in [(10, True), (200, True),
                                               (200, False), (10, False)]:
                                xt.XTestFakeKeyEvent(display, code, down, 0)
                            x.XSync(display, 0)
                        finally:
                            os.kill(self.process.pid, signal.SIGCONT)
                        time.sleep(0.4)
                        actual = self.title()
                        expected = 'RoutingKeys ' + ('字' if owned else '1字') + ' —'
                        details.append(dict(owned_repeat=owned, expected_prefix=expected,
                                            actual=actual, passed=actual.startswith(expected)))
        finally:
            x.XCloseDisplay(display)
        self.record('unmapped Unicode after queued shortcut repeats',
                    sum(not d['passed'] for d in details), len(details), details)

    def shift_and_altgr(self):
        details = []
        with self.window():
            self.text('TW!tw123')
            self.key('shift+Tab')
            actual = self.title()
            details.append(dict(expected='RoutingKeys TW!tw123Tab', actual=actual,
                                passed=actual.startswith('RoutingKeys TW!tw123Tab —')))
        # Native XKB AltGr, rather than pretending Ctrl+Alt text is plain input.
        subprocess.run(['setxkbmap', '-layout', 'de'], env=self.env, check=True,
                       capture_output=True)
        try:
            with self.window():
                self.xdo('keydown', 'ISO_Level3_Shift')
                self.key('q')
                self.xdo('keyup', 'ISO_Level3_Shift')
                time.sleep(0.15)
                actual = self.title()
                details.append(dict(expected='RoutingKeys @', actual=actual,
                                    passed=actual.startswith('RoutingKeys @ —')))
        finally:
            self.xdo('keyup', 'ISO_Level3_Shift')
            subprocess.run(['setxkbmap', '-layout', 'us'], env=self.env, check=True,
                           capture_output=True)
        self.record('ordinary Shift / native AltGr input',
                    sum(not d['passed'] for d in details), len(details), details)

    def rapid_address_editing(self):
        expected = ['seed', 'seed']
        with self.window():
            self.two_tabs(editing=True)
            for index in range(20):
                tab = index % 2
                self.xdo('keydown', 'ctrl+' + str(tab + 1))
                time.sleep(0.07)
                self.xdo('keyup', 'ctrl+' + str(tab + 1))
                self.text('tw1')
                expected[tab] += 'tw1'
            actual = []
            for index in range(2):
                self.select(index)
                actual.append(self.capture_editor())
        self.record('rapid switching with focused address editing',
                    sum(a != b for a, b in zip(actual, expected)), 2,
                    dict(switches=20, expected=expected, actual=actual))

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.xvfb.terminate()
        self.xvfb.wait(timeout=5)
        self.xlog.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary')
    parser.add_argument('output', type=Path)
    parser.add_argument('--trials', type=int, default=3)
    parser.add_argument('--case', choices=['all', 'slow', 'staggered', 'unicode', 'rapid', 'text'], default='all')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    harness = Harness(args.binary, args.output.resolve(), args.trials)
    try:
        if args.case in ('all', 'slow'):
            harness.slow_load()
        if args.case in ('all', 'staggered'):
            harness.staggered()
        if args.case in ('all', 'unicode'):
            harness.unicode_repeats()
        if args.case in ('all', 'text'):
            harness.shift_and_altgr()
        if args.case == 'all':
            harness.simultaneous(False)
            harness.simultaneous(True)
            harness.ordinary_after_release()
        if args.case in ('all', 'rapid'):
            harness.rapid_address_editing()
        (args.output / 'results.json').write_text(json.dumps(harness.results, indent=2) + '\n')
        return int(any(r['failures'] for r in harness.results))
    finally:
        harness.close()


if __name__ == '__main__':
    raise SystemExit(main())
