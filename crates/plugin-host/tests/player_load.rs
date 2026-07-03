//! End-to-end test of the MusicOS Player plugin: builds the cdylib, points
//! the project library at the checked-in corpus fixture, loads the plugin
//! through the CLAP host, drives the "Project" parameter, and asserts real
//! project audio comes out.

// The test exercises the unsafe loading API on a freshly built plugin.
#![allow(unsafe_code)]

use std::path::PathBuf;

use musicos_plugin_api::ProcessorPlugin;
use musicos_plugin_host::clap_host::ClapLibrary;

fn build_player_clap() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace = manifest_dir.join("../..").canonicalize().unwrap();
    let status = std::process::Command::new(env!("CARGO"))
        .args(["build", "-p", "musicos-player"])
        .current_dir(&workspace)
        .status()
        .expect("cargo runs");
    assert!(status.success(), "building musicos-player failed");

    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| workspace.join("target"), PathBuf::from);
    let built = target.join("debug").join(format!(
        "{}musicos_player{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    ));
    assert!(built.is_file(), "cdylib not found at {}", built.display());

    let dest = std::env::temp_dir().join(format!(
        "musicos-player-test-{}/MusicOS Player.clap",
        std::process::id()
    ));
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::copy(&built, &dest).unwrap();
    dest
}

/// Processes blocks until the async loader delivers audio (or times out).
fn energy_within(instance: &mut dyn ProcessorPlugin, timeout: std::time::Duration) -> f64 {
    let start = std::time::Instant::now();
    let mut l = vec![0.0f32; 512];
    let mut r = vec![0.0f32; 512];
    while start.elapsed() < timeout {
        l.fill(9.9); // prove the plugin overwrites, not accumulates
        r.fill(9.9);
        instance.process(&mut l, &mut r);
        assert!(l.iter().chain(r.iter()).all(|s| s.is_finite()));
        assert!(l.iter().chain(r.iter()).all(|s| s.abs() <= 1.0), "clipped");
        let energy: f64 = l.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
        if energy > 1e-6 {
            return energy;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    0.0
}

#[test]
fn player_lists_projects_as_a_param_and_plays_the_selection() {
    let corpus_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/v0.1.0")
        .canonicalize()
        .unwrap();
    // The library is every .musicos in MUSICOS_LIBRARY; ensure the single
    // env override is not set. Set before the plugin activates.
    // SAFETY: single-threaded at this point in the test.
    unsafe {
        std::env::remove_var("MUSICOS_PROJECT");
        std::env::set_var("MUSICOS_LIBRARY", &corpus_dir);
    }

    let path = build_player_clap();
    // SAFETY: loading our own freshly built plugin.
    let library = unsafe { ClapLibrary::load(&path) }.expect("player loads");

    let plugins = library.plugins().unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].id, "org.musicos.player");

    let mut instance = library.instantiate("org.musicos.player").unwrap();
    instance.prepare(48_000, 512); // activate: scans library, loads async

    // The picker is a real CLAP parameter surfaced through the host.
    let params = instance.params();
    assert_eq!(params.len(), 1, "expected the Project picker param");
    assert_eq!(params[0].name, "Project");
    assert!((params[0].min - 0.0).abs() < f32::EPSILON);
    assert!(params[0].max >= 0.0, "at least one library entry");
    let picker_id = params[0].id;

    // Loading is asynchronous: poll until the fixture audio arrives.
    let energy = energy_within(&mut instance, std::time::Duration::from_secs(10));
    assert!(energy > 1e-3, "player stayed silent — library scan failed?");

    // Re-selecting via the param (host-side set_param → CLAP flush) reloads
    // and keeps playing; unknown param ids are rejected.
    instance.set_param(picker_id, 0.0).unwrap();
    let energy = energy_within(&mut instance, std::time::Duration::from_secs(10));
    assert!(energy > 1e-3, "player silent after param re-select");
    assert!(instance.set_param("no_such_param", 1.0).is_err());

    // reset() rewinds the free-run cursor: two post-reset blocks match.
    instance.reset();
    let mut l0 = vec![0.0f32; 512];
    let mut r0 = vec![0.0f32; 512];
    instance.process(&mut l0, &mut r0);
    instance.reset();
    let mut l1 = vec![0.0f32; 512];
    let mut r1 = vec![0.0f32; 512];
    instance.process(&mut l1, &mut r1);
    assert_eq!(l0, l1);
    assert_eq!(r0, r1);
}
