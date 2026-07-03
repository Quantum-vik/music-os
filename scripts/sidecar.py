#!/usr/bin/env python3
"""Prepares the music-server sidecar for Tauri bundling.

Tauri's `bundle.externalBin` expects `binaries/music-server-<target-triple>`
next to the desktop crate; at bundle time it lands as `music-server` beside
the app executable, which is exactly where `find_server_binary` looks first.
Run from anywhere; used as the desktop `beforeBuildCommand` and in CI.
"""

import pathlib
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent


def target_triple() -> str:
    out = subprocess.run(
        ["rustc", "-vV"], capture_output=True, text=True, check=True
    ).stdout
    for line in out.splitlines():
        if line.startswith("host:"):
            return line.split()[1]
    sys.exit("rustc -vV did not report a host triple")


def main() -> None:
    triple = target_triple()
    exe = ".exe" if sys.platform == "win32" else ""
    subprocess.run(
        ["cargo", "build", "--release", "-p", "musicos-server"],
        cwd=ROOT,
        check=True,
    )
    built = ROOT / "target" / "release" / f"music-server{exe}"
    dest_dir = ROOT / "apps" / "desktop" / "binaries"
    dest_dir.mkdir(parents=True, exist_ok=True)
    dest = dest_dir / f"music-server-{triple}{exe}"
    shutil.copy2(built, dest)
    print(f"sidecar ready: {dest}")


if __name__ == "__main__":
    main()
