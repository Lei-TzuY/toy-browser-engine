# Build, install and reproduce the Research Preview

Use trusted content on a development machine. This application has no verified
renderer sandbox; these instructions are not safe arbitrary-web browsing advice.

## Targets and prerequisites

- Linux x86-64: measured on Ubuntu 26.04 under WSL2, Rust 1.98.0. The shipped
  binary uses this environment's glibc/shared libraries. Older distributions are
  not binary-compatibility promises; build from source there.
- Native Windows x86-64: measured on Windows 11, MSVC Rust 1.96.0. Source builds
  need the Rust MSVC toolchain and Microsoft C++ build tools. System DLLs/UCRT and
  installed Segoe UI/Arial fonts are used. Runtime import/launch receipts accompany
  packages. WSL does not satisfy the Windows gate.
- Python 3.11+ is needed for packaging/CLI verification. Windows GUI smoke uses
  Python's standard library, Win32 and an already loaded US English layout. It
  targets its own browser window and restores its original input context.
- Ubuntu build prerequisites include `build-essential pkg-config libx11-dev
  libxcursor-dev libxrandr-dev libxi-dev libwayland-dev libxkbcommon-dev
  fonts-dejavu-core`.
- X11 GUI tests additionally need Xvfb, xdotool, x11-xkb-utils, libXtst, ImageMagick
  and Pillow. These are test tools, not bundled runtime services.

Exact OS/toolchain/library receipts are measurements, not an MSRV or all-platform
certification. Native macOS and live Wayland GUI are not release-verified.

## Build from source

```sh
git clone https://github.com/Lei-TzuY/toy-browser-engine.git
cd toy-browser-engine
# Check out the exact release tag/candidate SHA named in its receipt.
cargo build --locked
cargo test --locked
cargo build --release --locked
target/release/browser_engine --quiet --screenshot demo.ppm
target/release/browser_engine --window --quiet
```

On Windows use `.\target\release\browser_engine.exe`. Run from a writable directory.
The first screenshot/window uses the built-in demo with no public network.
On Wayland/WSLg, the tested X11 path can be selected per process with
`env -u WAYLAND_DISPLAY target/release/browser_engine --window --quiet` when X11
is available. This does not validate the separate Wayland backend. Use US English
for the measured Windows keyboard path; general IME/layout support is incomplete.

## Download and install

Use assets only from the repository's verified GitHub Release URL. Packages are
unsigned and experimental. Do not weaken SmartScreen, antivirus, firewall or
other host protections; building from source is an alternative if policy blocks
a downloaded executable.

1. Download the matching ZIP/TAR.GZ and `SHA256SUMS`.
2. Check SHA-256 before extraction: `sha256sum <archive>` on Linux or
   `Get-FileHash .\<archive>.zip -Algorithm SHA256` in PowerShell. Compare the
   published value. `sha256sum -c SHA256SUMS` works when both listed archives exist.
3. Extract into a new writable user directory. No installer/admin access is needed.
   Keep `LICENSE`, `THIRD-PARTY-NOTICES.json` and `third-party/`.
4. Run `./browser_engine --window --quiet` or
   `.\browser_engine.exe --window --quiet` for the trusted built-in demo.
5. Close normally. Uninstall by deleting only this extracted directory after exit.
   No service/auto-updater is installed; state is not a production persistent profile.

`BUILD.json` records source SHA, target, toolchain, lockfile/binary hashes.
`CONTENT_SHA256SUMS` covers unpacked files. Extraction/native executable receipts
prove delivered bytes, not byte-for-byte reproducible compilation.

## Reproduce smoke tests

```sh
python3 scripts/smoke_release.py target/release/browser_engine /tmp/browser-cli
# With a free, private Xvfb display already running:
env -u WAYLAND_DISPLAY DISPLAY=:99 python3 tests/gui/tabs_window.py \
  target/release/browser_engine /tmp/browser-tabs
python3 tests/gui/shortcut_routing.py target/release/browser_engine \
  /tmp/browser-routing --trials 6
```

Never attach automation to an unrelated desktop. The routing script creates and
closes its own Xvfb display. GUI delays synchronize OS events; they are not
application input-routing heuristics.

```powershell
python scripts/smoke_release.py target/release/browser_engine.exe native-cli
python tests/gui/windows_smoke.py target/release/browser_engine.exe native-gui --latin-input
```

The Windows test checks foreground ownership before key-down, uses loopback HTTP,
captures only its own window, and cleans up its process/input context. Native GUI
receipts are separate from CI compilation/CLI success, live IME or physical-hardware
certification.

## Package an exact candidate

Start with a clean tracked checkout at the candidate SHA; build/test first.

```sh
python3 scripts/package_release.py --binary target/release/browser_engine \
  --metadata metadata.json --revision <exact-candidate-sha> \
  --target x86_64-unknown-linux-gnu --output dist
```

On native Windows use `python`, the `.exe` and target `x86_64-pc-windows-msvc`.
Packaging preserves dependency license texts, includes Rust copyright notices,
checks archive contents after extraction and runs the extracted executable against
the demo and loopback 404 fixture. Missing license texts or failed extraction/launch
are release blockers.

The packager generates `metadata.json` itself using locked Cargo metadata filtered
for the native target, and follows the resolved root dependency closure. It does
not redistribute unrelated UEFI/Redox/Wasm platform crates merely because they
appear in Cargo.lock. License checks remain mandatory for every selected package.
