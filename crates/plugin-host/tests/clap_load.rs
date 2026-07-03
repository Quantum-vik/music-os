//! End-to-end CLAP hosting test: builds the `musicos-clap-gain` cdylib with
//! cargo, renames it to `.clap`, and drives it through the full host path —
//! dlopen → `clap_entry` → factory → instance → activate → process.

// The test exercises the unsafe loading API on a freshly built plugin.
#![allow(unsafe_code)]

use std::path::PathBuf;

use musicos_plugin_api::ProcessorPlugin;
use musicos_plugin_host::clap_host::{ClapHostError, ClapLibrary};

/// Builds the test plugin and returns a copy renamed to `.clap`.
fn build_test_clap() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest_dir.join("../..").canonicalize().unwrap();
    let status = std::process::Command::new(env!("CARGO"))
        .args(["build", "-p", "musicos-clap-gain"])
        .current_dir(&workspace)
        .status()
        .expect("cargo runs");
    assert!(status.success(), "building musicos-clap-gain failed");

    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| workspace.join("target"), PathBuf::from);
    let built = target.join("debug").join(format!(
        "{}musicos_clap_gain{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    assert!(built.is_file(), "cdylib not found at {}", built.display());

    let dest = std::env::temp_dir().join(format!(
        "musicos-clap-test-{}/MusicOS Test Gain.clap",
        std::process::id()
    ));
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::copy(&built, &dest).unwrap();
    dest
}

#[test]
fn loads_lists_and_processes_a_real_clap_binary() {
    let path = build_test_clap();
    // SAFETY: loading our own freshly built test plugin.
    let library = unsafe { ClapLibrary::load(&path) }.expect("library loads");

    let plugins = library.plugins().expect("factory lists plugins");
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].id, "org.musicos.test-gain");
    assert_eq!(plugins[0].name, "MusicOS Test Gain");

    assert!(matches!(
        library.instantiate("org.musicos.no-such-plugin"),
        Err(ClapHostError::UnknownId(_))
    ));

    let mut instance = library.instantiate("org.musicos.test-gain").unwrap();
    assert_eq!(instance.info().vendor, "MusicOS");
    instance.prepare(48_000, 512);

    // The ProcessorPlugin adapter must apply the plugin's -6 dB gain.
    let mut left = vec![1.0f32; 512];
    let mut right = vec![-0.5f32; 512];
    instance.process(&mut left, &mut right);
    assert!(left.iter().all(|s| (*s - 0.5).abs() < 1e-6));
    assert!(right.iter().all(|s| (*s + 0.25).abs() < 1e-6));

    // Odd block sizes below the prepared maximum work too.
    let mut left = vec![0.8f32; 33];
    let mut right = vec![0.8f32; 33];
    instance.process(&mut left, &mut right);
    assert!(left.iter().all(|s| (*s - 0.4).abs() < 1e-6));

    // Descriptor adapts CLAP metadata to the native descriptor shape.
    let descriptor = instance.descriptor();
    assert_eq!(descriptor.id, "org.musicos.test-gain");

    // Params are not surfaced yet: every id is unknown.
    assert!(instance.set_param("gain", 0.2).is_err());

    drop(instance);
    let missing = std::env::temp_dir().join("musicos-definitely-not-a-plugin.clap");
    assert!(matches!(
        unsafe { ClapLibrary::load(&missing) },
        Err(ClapHostError::Load(_))
    ));
}
