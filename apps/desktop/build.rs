//! Tauri build glue.
//!
//! `bundle.externalBin` makes `tauri_build` require the `music-server` sidecar
//! at compile time, but plain `cargo build`/CI never runs the bundler. A
//! placeholder satisfies the path check; real bundling always overwrites it
//! first via `beforeBuildCommand` (scripts/sidecar.py).

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    let suffix = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let sidecar = std::path::PathBuf::from(format!("binaries/music-server-{target}{suffix}"));
    if !sidecar.exists() {
        std::fs::create_dir_all("binaries").expect("create binaries dir");
        std::fs::write(&sidecar, b"").expect("write sidecar placeholder");
    }
    tauri_build::build();
}
