#!/usr/bin/env python3
"""Package one tested native binary, locked dependency notices, and research warnings.

Requires Python 3.11+, a clean candidate checkout, and Cargo's native target graph.
This script neither publishes a release nor claims reproducible binary builds.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import struct
import subprocess
import tarfile
import tempfile
import tomllib
import zipfile

from smoke_release import smoke


ROOT = Path(__file__).resolve().parents[1]
PAYLOAD = [
    "README.md", "LICENSE", "Cargo.toml", "Cargo.lock", "STABILIZATION.md",
    "docs/release-readiness.md", "docs/release-notes-0.1.0.md", "docs/build-and-run.md",
    "docs/pr-inventory.json", "scripts/smoke_release.py", "scripts/package_release.py",
    "tests/gui/windows_smoke.py", "tests/gui/shortcut_routing.py", "tests/gui/tabs_window.py",
]


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def command(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True, encoding="utf-8").strip()


def runtime_dependencies(binary, windows):
    if not windows:
        return subprocess.check_output(["ldd", str(binary)], text=True, encoding="utf-8")
    raw = binary.read_bytes()
    pe = struct.unpack_from("<I", raw, 0x3c)[0]
    assert raw[pe:pe + 4] == b"PE\0\0" and struct.unpack_from("<H", raw, pe + 4)[0] == 0x8664
    count, optional_size = struct.unpack_from("<H", raw, pe + 6)[0], struct.unpack_from("<H", raw, pe + 20)[0]
    optional = pe + 24
    assert struct.unpack_from("<H", raw, optional)[0] == 0x20b
    sections = optional + optional_size

    def offset(rva):
        for index in range(count):
            section = sections + 40 * index
            size, address, disk_size, disk = struct.unpack_from("<IIII", raw, section + 8)
            if address <= rva < address + max(size, disk_size):
                return disk + rva - address
        raise ValueError("PE RVA outside sections")

    table = offset(struct.unpack_from("<I", raw, optional + 112 + 8)[0])
    names = []
    while any(raw[table:table + 20]):
        name = offset(struct.unpack_from("<I", raw, table + 12)[0])
        names.append(raw[name:raw.index(b"\0", name)].decode("ascii"))
        table += 20
    return sorted(names)


def notices(metadata, stage, target_triple):
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    selected, pending = set(), [metadata["resolve"]["root"]]
    while pending:
        package_id = pending.pop()
        if package_id in selected:
            continue
        selected.add(package_id)
        pending.extend(dep["pkg"] for dep in nodes[package_id]["deps"])
    index = []
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if package["id"] not in selected or package["name"] == "browser_engine":
            continue  # the project license is a separate top-level payload
        if not package.get("license"):
            raise ValueError("unresolved license metadata: " + package["name"])
        source = Path(package["manifest_path"]).parent.resolve()
        candidates = {p for p in source.rglob("*") if p.is_file() and (
            p.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE", "UNLICENSE"))
            or any(part.upper() in ("LICENSES", "LICENCES") for part in p.relative_to(source).parts[:-1]))}
        if package.get("license_file"):
            candidates.add(source / package["license_file"])
        if not candidates:
            raise ValueError("no redistributable license text: " + package["name"])
        destination = stage / "third-party" / (package["name"] + "-" + package["version"])
        entries = []
        for path in sorted(candidates):
            resolved = path.resolve()
            if not resolved.is_relative_to(source):
                raise ValueError("license outside package: " + str(path))
            relative = path.relative_to(source)
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, target)
            entries.append({"path": target.relative_to(stage).as_posix(), "sha256": sha256(target)})
        index.append({"name": package["name"], "version": package["version"],
                      "license_expression": package["license"], "repository": package.get("repository"),
                      "source": package.get("source"), "license_files": entries})

    # Rust's linked standard library has its own copyright/dependency notices.
    sysroot = Path(command("rustc", "--print", "sysroot"))
    rust_docs = sysroot / "share" / "doc" / "rust"
    rust_files = [p for p in rust_docs.iterdir() if p.is_file() and
                  p.name.upper().startswith(("COPYRIGHT", "LICENSE"))] if rust_docs.exists() else []
    if (rust_docs / "licenses").exists():
        rust_files.extend(p for p in (rust_docs / "licenses").rglob("*") if p.is_file())
    if not any(p.name.upper().startswith("COPYRIGHT") for p in rust_files):
        raise ValueError("Rust standard-library copyright notices unavailable in toolchain")
    for path in rust_files:
        target = stage / "third-party" / "rust-toolchain" / path.relative_to(rust_docs)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(path, target)
    (stage / "THIRD-PARTY-NOTICES.json").write_text(json.dumps({
        "scope": "Native target dependency closure from Cargo --filter-platform, including build dependencies. Other locked platforms are not shipped. Original license expressions/texts are preserved; no claim of legal certification.",
        "target": target_triple,
        "packages": index,
        "rust_toolchain": command("rustc", "-Vv"),
        "rust_notices": [p.relative_to(rust_docs).as_posix() for p in rust_files],
    }, indent=2) + "\n", encoding="utf-8")
    return len(index)


def extract(archive, destination):
    def safe(name):
        relative = PurePosixPath(name)
        if relative.is_absolute() or ".." in relative.parts or "\\" in name:
            raise ValueError("unsafe archive member: " + name)
        return destination.joinpath(*relative.parts)

    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as package:
            for info in package.infolist():
                target = safe(info.filename)
                if (info.external_attr >> 16) & 0o170000 == 0o120000:
                    raise ValueError("symlink not allowed")
                if info.is_dir():
                    target.mkdir(parents=True, exist_ok=True)
                else:
                    target.parent.mkdir(parents=True, exist_ok=True)
                    with package.open(info) as source, target.open("wb") as output:
                        shutil.copyfileobj(source, output)
    else:
        with tarfile.open(archive, "r:gz") as package:
            for info in package.getmembers():
                target = safe(info.name)
                if info.isdir():
                    target.mkdir(parents=True, exist_ok=True)
                elif info.isfile():
                    target.parent.mkdir(parents=True, exist_ok=True)
                    with package.extractfile(info) as source, target.open("wb") as output:
                        shutil.copyfileobj(source, output)
                    target.chmod(info.mode & 0o777)
                else:
                    raise ValueError("links and special files not allowed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--metadata", type=Path, required=True, help="write the filtered native Cargo metadata receipt here")
    parser.add_argument("--revision", required=True)
    parser.add_argument("--target", choices=["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"], required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    assert command("git", "rev-parse", "HEAD") == args.revision
    assert not command("git", "status", "--porcelain", "--untracked-files=no"), "tracked source is dirty"
    for relative in PAYLOAD:
        command("git", "ls-files", "--error-unmatch", relative)
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    windows = "windows" in args.target
    assert windows == (os.name == "nt"), "package/run on its native platform"
    native_metadata = subprocess.check_output([
        "cargo", "metadata", "--locked", "--filter-platform", args.target,
        "--format-version", "1"], cwd=ROOT)
    args.metadata.parent.mkdir(parents=True, exist_ok=True)
    args.metadata.write_bytes(native_metadata)
    args.output.mkdir(parents=True, exist_ok=True)
    stem = "toy-browser-engine-v" + version + "-research-preview-" + args.target
    binary_name = "browser_engine.exe" if windows else "browser_engine"
    with tempfile.TemporaryDirectory(prefix="toy-browser-package-") as temp:
        stage = Path(temp) / stem
        stage.mkdir()
        for relative in PAYLOAD:
            target = stage / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / relative, target)
        shutil.copyfile(args.binary, stage / binary_name)
        (stage / binary_name).chmod(0o755)
        count = notices(json.loads(native_metadata), stage, args.target)
        build = {"version": version, "label": "Research Preview", "source_revision": args.revision,
                 "target": args.target, "rustc": command("rustc", "-Vv"), "cargo": command("cargo", "-V"),
                 "created_at_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                 "binary_sha256": sha256(stage / binary_name), "cargo_lock_sha256": sha256(ROOT / "Cargo.lock"),
                 "dependency_notice_packages": count, "signed": False,
                 "runtime_dependencies": runtime_dependencies(stage / binary_name, windows),
                 "reproducible_binary_build_proven": False}
        (stage / "BUILD.json").write_text(json.dumps(build, indent=2) + "\n", encoding="utf-8")
        files = sorted(p for p in stage.rglob("*") if p.is_file())
        (stage / "CONTENT_SHA256SUMS").write_text("".join(
            sha256(p) + "  " + p.relative_to(stage).as_posix() + "\n" for p in files), encoding="utf-8")
        archive = args.output / (stem + (".zip" if windows else ".tar.gz"))
        if windows:
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as package:
                for path in sorted(stage.rglob("*")):
                    if path.is_file():
                        package.write(path, path.relative_to(stage.parent).as_posix())
        else:
            with tarfile.open(archive, "w:gz") as package:
                package.add(stage, arcname=stem)

    # Verify the archive, not the pre-archive staging directory.
    extracted = args.output / (stem + "-verified")
    if extracted.exists():
        raise FileExistsError("verification output already exists: " + str(extracted))
    extracted.mkdir()
    extract(archive, extracted)
    root = extracted / stem
    entries = (root / "CONTENT_SHA256SUMS").read_text(encoding="utf-8").splitlines()
    for line in entries:
        expected, relative = line.split("  ", 1)
        assert sha256(root / relative) == expected, relative
    assert sha256(root / binary_name) == build["binary_sha256"]
    result = smoke(root / binary_name, args.output / (stem + "-smoke"))
    receipt = {"archive": str(archive.resolve()), "size_bytes": archive.stat().st_size,
               "sha256": sha256(archive), "source_revision": args.revision,
               "content_files_verified": len(entries), "extracted_binary": str(root / binary_name),
               "binary_sha256": build["binary_sha256"], "dependency_notice_packages": count,
               "native_cli_smoke": result}
    (args.output / (stem + ".receipt.json")).write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    (args.output / (archive.name + ".sha256")).write_text(receipt["sha256"] + "  " + archive.name + "\n", encoding="utf-8")
    print(json.dumps(receipt, indent=2))


if __name__ == "__main__":
    main()
