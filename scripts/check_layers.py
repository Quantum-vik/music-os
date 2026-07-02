#!/usr/bin/env python3
"""Enforce the architecture dependency rule (docs/02_System_Architecture.md §2).

Every internal crate is assigned a layer. A crate may depend only on internal
crates in the SAME or a LOWER layer. Unknown crates fail loudly so new crates
must be classified here (a deliberate speed bump: placing a crate in the layer
map is an architectural decision).

Run: python3 scripts/check_layers.py   (needs `cargo` on PATH)
"""

import json
import subprocess
import sys

# Layer map. Lower number = closer to the core. See docs/02 §2-3.
LAYERS = {
    # 0 — leaf types
    "musicos-core-types": 0,
    # 1 — pure domain
    "musicos-music-core": 1,
    "musicos-harmony": 1,
    "musicos-rhythm": 1,
    "musicos-timeline": 1,
    "musicos-events": 1,
    # 2 — domain composites & ports
    "musicos-midi": 2,
    "musicos-project-model": 2,
    "musicos-audio-graph": 2,
    "musicos-dsp": 2,
    "musicos-instruments": 2,
    "musicos-plugin-api": 2,
    "musicos-composition": 2,
    "musicos-arrangement": 2,
    "musicos-config": 2,
    "musicos-telemetry": 2,
    # 3 — application services & engines
    "musicos-project-service": 3,
    "musicos-storage": 3,
    "musicos-render": 3,
    "musicos-audio-engine": 3,
    "musicos-plugin-host": 3,
    "musicos-ai-runtime": 3,
    "musicos-tools": 3,
    # 4 — protocol surfaces & provider adapters
    "musicos-ai-providers": 4,
    "musicos-mcp-server": 4,
    "musicos-sdk": 4,
    # 5 — apps (composition roots: may name anything below)
    "musicos-cli": 5,
    "musicos-server": 5,
}


def main() -> int:
    meta = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--no-deps"]
        )
    )
    internal = {p["name"]: p for p in meta["packages"]}
    errors = []

    for name, pkg in sorted(internal.items()):
        if name not in LAYERS:
            errors.append(f"{name}: not in the layer map — classify it in scripts/check_layers.py")
            continue
        for dep in pkg["dependencies"]:
            dep_name = dep["name"]
            if dep_name not in internal:
                continue  # external crates are governed by cargo-deny, not layers
            if dep_name not in LAYERS:
                errors.append(f"{name} → {dep_name}: dependency not in the layer map")
            elif LAYERS[dep_name] > LAYERS[name]:
                errors.append(
                    f"{name} (layer {LAYERS[name]}) depends on {dep_name} "
                    f"(layer {LAYERS[dep_name]}) — dependencies must point inward"
                )

    if errors:
        print("layer check FAILED:", file=sys.stderr)
        for e in errors:
            print(f"  - {e}", file=sys.stderr)
        return 1
    print(f"layer check OK ({len(internal)} crates)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
