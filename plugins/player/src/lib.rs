//! MusicOS Player: a CLAP plugin that plays a `.musicos` project inside any
//! host DAW (FL Studio, Bitwig, Reaper, ...).
//!
//! The DAW-bridge strategy (docs/12 post-v1 backlog, pulled forward): rather
//! than exporting to proprietary project formats, MusicOS itself becomes a
//! plugin. On activation the plugin loads the project named by the
//! `MUSICOS_PROJECT` environment variable (or the first line of
//! `~/.musicos/player-project.txt`), renders it deterministically at the
//! host's sample rate, and plays it back **synced to the host transport**
//! (seconds timeline preferred, beats+tempo fallback, free-run when the host
//! provides no transport). For VST3-only hosts, wrap the built `.clap` with
//! the free-audio `clap-wrapper` project — no VST3 SDK enters this tree.

// Implementing the CLAP C ABI requires unsafe throughout; the plugin keeps
// every unsafe block small and auditable.
#![allow(unsafe_code)]

use std::ffi::{c_char, c_void, CStr};

use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    clap_event_transport, CLAP_TRANSPORT_HAS_SECONDS_TIMELINE, CLAP_TRANSPORT_HAS_TEMPO,
    CLAP_TRANSPORT_IS_PLAYING,
};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::fixedpoint::{CLAP_BEATTIME_FACTOR, CLAP_SECTIME_FACTOR};
use clap_sys::host::clap_host;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::{clap_process, clap_process_status, CLAP_PROCESS_CONTINUE};
use clap_sys::version::CLAP_VERSION;
use musicos_render::RenderOptions;

const ID: &CStr = c"org.musicos.player";
const NAME: &CStr = c"MusicOS Player";
const VENDOR: &CStr = c"MusicOS";
const VERSION: &CStr = c"0.1.0";
const EMPTY: &CStr = c"";

/// Wrapper making pointer-holding CLAP statics `Sync`; the pointed-to data
/// is immutable `'static` strings, so cross-thread reads are safe.
#[repr(transparent)]
struct SyncWrap<T>(T);
// SAFETY: only wraps immutable static data.
unsafe impl<T> Sync for SyncWrap<T> {}

static FEATURES: SyncWrap<[*const c_char; 2]> =
    SyncWrap([c"instrument".as_ptr(), std::ptr::null()]);

static DESCRIPTOR: SyncWrap<clap_plugin_descriptor> = SyncWrap(clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: ID.as_ptr(),
    name: NAME.as_ptr(),
    vendor: VENDOR.as_ptr(),
    url: EMPTY.as_ptr(),
    manual_url: EMPTY.as_ptr(),
    support_url: EMPTY.as_ptr(),
    version: VERSION.as_ptr(),
    description: EMPTY.as_ptr(),
    features: FEATURES.0.as_ptr(),
});

/// Plugin instance state (behind `clap_plugin.plugin_data`).
struct Player {
    /// Pre-rendered project audio at the host sample rate.
    left: Vec<f32>,
    right: Vec<f32>,
    sample_rate: f64,
    /// Cursor for hosts that provide no transport.
    free_pos: usize,
}

impl Player {
    /// Resolves the project path: `MUSICOS_PROJECT` env var, then the first
    /// line of `~/.musicos/player-project.txt`.
    fn project_path() -> Option<std::path::PathBuf> {
        if let Ok(p) = std::env::var("MUSICOS_PROJECT") {
            if !p.trim().is_empty() {
                return Some(std::path::PathBuf::from(p.trim()));
            }
        }
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
        let cfg = std::path::PathBuf::from(home).join(".musicos/player-project.txt");
        let text = std::fs::read_to_string(cfg).ok()?;
        let line = text.lines().next()?.trim();
        (!line.is_empty()).then(|| std::path::PathBuf::from(line))
    }

    /// Loads and renders the configured project. Failures leave the player
    /// silent (a plugin must not crash its host over a missing file).
    fn load(&mut self, sample_rate: f64) {
        self.sample_rate = sample_rate;
        self.left.clear();
        self.right.clear();
        self.free_pos = 0;
        let Some(path) = Self::project_path() else {
            eprintln!("MusicOS Player: no project configured (set MUSICOS_PROJECT)");
            return;
        };
        let rendered = musicos_storage::BundleStore::open(&path)
            .map_err(|e| e.to_string())
            .and_then(|store| store.load_state().map_err(|e| e.to_string()))
            .and_then(|state| {
                let opts = RenderOptions {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    sample_rate: sample_rate as u32,
                    ..RenderOptions::default()
                };
                musicos_render::render_project(&state, &opts).map_err(|e| e.to_string())
            });
        match rendered {
            Ok(buffer) => {
                self.left = buffer.left;
                self.right = buffer.right;
            }
            Err(e) => eprintln!("MusicOS Player: cannot load {}: {e}", path.display()),
        }
    }

    /// The playback position for this block, honoring host transport.
    /// Returns `None` when the host transport is stopped.
    fn position(&mut self, transport: *const clap_event_transport, frames: usize) -> Option<usize> {
        if transport.is_null() {
            let pos = self.free_pos;
            self.free_pos += frames;
            return Some(pos);
        }
        let t = unsafe { &*transport };
        if t.flags & CLAP_TRANSPORT_IS_PLAYING == 0 {
            return None;
        }
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        #[allow(clippy::cast_sign_loss)]
        if t.flags & CLAP_TRANSPORT_HAS_SECONDS_TIMELINE != 0 {
            let seconds = t.song_pos_seconds as f64 / CLAP_SECTIME_FACTOR as f64;
            Some((seconds.max(0.0) * self.sample_rate) as usize)
        } else if t.flags & CLAP_TRANSPORT_HAS_TEMPO != 0 && t.tempo > 0.0 {
            let beats = t.song_pos_beats as f64 / CLAP_BEATTIME_FACTOR as f64;
            Some((beats.max(0.0) * 60.0 / t.tempo * self.sample_rate) as usize)
        } else {
            let pos = self.free_pos;
            self.free_pos += frames;
            Some(pos)
        }
    }

    /// Fills one output block from the pre-rendered audio.
    fn fill(&mut self, transport: *const clap_event_transport, l: &mut [f32], r: &mut [f32]) {
        let frames = l.len().min(r.len());
        l[..frames].fill(0.0);
        r[..frames].fill(0.0);
        let Some(start) = self.position(transport, frames) else {
            return;
        };
        if start >= self.left.len() {
            return;
        }
        let n = frames.min(self.left.len() - start);
        l[..n].copy_from_slice(&self.left[start..start + n]);
        r[..n].copy_from_slice(&self.right[start..start + n]);
    }
}

fn player_of(plugin: *const clap_plugin) -> *mut Player {
    unsafe { (*plugin).plugin_data.cast::<Player>() }
}

unsafe extern "C" fn plugin_init(_plugin: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn plugin_destroy(plugin: *const clap_plugin) {
    if plugin.is_null() {
        return;
    }
    let data = player_of(plugin);
    if !data.is_null() {
        drop(unsafe { Box::from_raw(data) });
    }
    drop(unsafe { Box::from_raw(plugin.cast_mut()) });
}

unsafe extern "C" fn plugin_activate(
    plugin: *const clap_plugin,
    sample_rate: f64,
    _min_frames: u32,
    _max_frames: u32,
) -> bool {
    unsafe { &mut *player_of(plugin) }.load(sample_rate);
    true
}

unsafe extern "C" fn plugin_deactivate(_plugin: *const clap_plugin) {}

unsafe extern "C" fn plugin_start_processing(_plugin: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn plugin_stop_processing(_plugin: *const clap_plugin) {}

unsafe extern "C" fn plugin_reset(plugin: *const clap_plugin) {
    unsafe { &mut *player_of(plugin) }.free_pos = 0;
}

unsafe extern "C" fn plugin_process(
    plugin: *const clap_plugin,
    process: *const clap_process,
) -> clap_process_status {
    let process = unsafe { &*process };
    let frames = process.frames_count as usize;
    if process.audio_outputs_count >= 1 {
        let output = unsafe { &*process.audio_outputs };
        if output.channel_count >= 2 {
            let l = unsafe { std::slice::from_raw_parts_mut(*output.data32, frames) };
            let r = unsafe { std::slice::from_raw_parts_mut(*output.data32.add(1), frames) };
            unsafe { &mut *player_of(plugin) }.fill(process.transport, l, r);
        }
    }
    CLAP_PROCESS_CONTINUE
}

unsafe extern "C" fn plugin_get_extension(
    _plugin: *const clap_plugin,
    _id: *const c_char,
) -> *const c_void {
    std::ptr::null()
}

unsafe extern "C" fn plugin_on_main_thread(_plugin: *const clap_plugin) {}

unsafe extern "C" fn factory_get_plugin_count(_factory: *const clap_plugin_factory) -> u32 {
    1
}

unsafe extern "C" fn factory_get_plugin_descriptor(
    _factory: *const clap_plugin_factory,
    index: u32,
) -> *const clap_plugin_descriptor {
    if index == 0 {
        &raw const DESCRIPTOR.0
    } else {
        std::ptr::null()
    }
}

unsafe extern "C" fn factory_create_plugin(
    _factory: *const clap_plugin_factory,
    _host: *const clap_host,
    plugin_id: *const c_char,
) -> *const clap_plugin {
    if plugin_id.is_null() || unsafe { CStr::from_ptr(plugin_id) } != ID {
        return std::ptr::null();
    }
    let player = Box::into_raw(Box::new(Player {
        left: Vec::new(),
        right: Vec::new(),
        sample_rate: 48_000.0,
        free_pos: 0,
    }));
    Box::into_raw(Box::new(clap_plugin {
        desc: &raw const DESCRIPTOR.0,
        plugin_data: player.cast(),
        init: Some(plugin_init),
        destroy: Some(plugin_destroy),
        activate: Some(plugin_activate),
        deactivate: Some(plugin_deactivate),
        start_processing: Some(plugin_start_processing),
        stop_processing: Some(plugin_stop_processing),
        reset: Some(plugin_reset),
        process: Some(plugin_process),
        get_extension: Some(plugin_get_extension),
        on_main_thread: Some(plugin_on_main_thread),
    }))
}

static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(factory_get_plugin_count),
    get_plugin_descriptor: Some(factory_get_plugin_descriptor),
    create_plugin: Some(factory_create_plugin),
};

unsafe extern "C" fn entry_init(_path: *const c_char) -> bool {
    true
}

unsafe extern "C" fn entry_deinit() {}

unsafe extern "C" fn entry_get_factory(factory_id: *const c_char) -> *const c_void {
    if !factory_id.is_null() && unsafe { CStr::from_ptr(factory_id) } == CLAP_PLUGIN_FACTORY_ID {
        (&raw const FACTORY).cast()
    } else {
        std::ptr::null()
    }
}

/// The CLAP entry point resolved by hosts after dlopen.
#[allow(non_upper_case_globals)]
#[no_mangle]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_get_factory),
};
