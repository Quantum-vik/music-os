#!/usr/bin/env python3
"""Prepares the music-server resource for Tauri bundling.

Copied to `apps/desktop/binaries/music-server[.exe]` and bundled via
`bundle.resources`; the desktop app resolves it at runtime through Tauri's
Resource path (with `find_server_binary` as the dev-mode fallback).
Run from anywhere; used as the desktop `beforeBuildCommand`.
"""

import pathlib
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent


def main() -> None:
    exe = ".exe" if sys.platform == "win32" else ""
    subprocess.run(
        ["cargo", "build", "--release", "-p", "musicos-server"],
        cwd=ROOT,
        check=True,
    )
    built = ROOT / "target" / "release" / f"music-server{exe}"
    dest_dir = ROOT / "apps" / "desktop" / "binaries"
    dest_dir.mkdir(parents=True, exist_ok=True)
    dest = dest_dir / f"music-server{exe}"
    shutil.copy2(built, dest)
    print(f"sidecar ready: {dest}")


if __name__ == "__main__":
    main()
