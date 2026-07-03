//! End-to-end test of the MusicOS Player plugin: builds the cdylib, points
//! `MUSICOS_PROJECT` at the checked-in corpus fixture, loads it through the
//! CLAP host, and asserts real project audio comes out.

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

#[test]
fn player_plays_the_configured_project_in_a_host() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/v0.1.0/Fixture.musicos")
        .canonicalize()
        .unwrap();
    // Set before the plugin activates; the plugin reads it at activate time.
    // SAFETY: single-threaded at this point in the test.
    unsafe { std::env::set_var("MUSICOS_PROJECT", &fixture) };

    let path = build_player_clap();
    // SAFETY: loading our own freshly built plugin.
    let library = unsafe { ClapLibrary::load(&path) }.expect("player loads");

    let plugins = library.plugins().unwrap();
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].id, "org.musicos.player");
    assert_eq!(plugins[0].name, "MusicOS Player");

    let mut instance = library.instantiate("org.musicos.player").unwrap();
    instance.prepare(48_000, 512); // activate: loads + renders the fixture

    // Our host provides no transport, so the player free-runs from frame 0.
    // The fixture has chords at bar 0 — audio must be non-silent and finite.
    let mut energy = 0.0f64;
    let mut l = vec![0.0f32; 512];
    let mut r = vec![0.0f32; 512];
    for _ in 0..20 {
        l.fill(9.9); // prove the plugin overwrites, not accumulates
        r.fill(9.9);
        instance.process(&mut l, &mut r);
        assert!(l.iter().chain(r.iter()).all(|s| s.is_finite()));
        assert!(l.iter().chain(r.iter()).all(|s| s.abs() <= 1.0), "clipped");
        energy += l.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>();
    }
    assert!(
        energy > 1e-3,
        "player produced silence (energy {energy:.6}) — project not loaded?"
    );

    // reset() rewinds the free-run cursor: the next block matches block 0.
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
