//! MusicOS Player: a CLAP plugin that plays a `.musicos` project inside any
//! host DAW (FL Studio, Bitwig, Reaper, ...).
//!
//! The DAW-bridge strategy (docs/12 post-v1 backlog, pulled forward): rather
//! than exporting to proprietary project formats, MusicOS itself becomes a
//! plugin. The plugin scans a **project library** (`MUSICOS_LIBRARY` env var
//! or `~/.musicos/projects`, plus the `MUSICOS_PROJECT` env override) and
//! exposes a stepped **"Project" parameter** (CLAP params extension) so the
//! song is picked inside the DAW. Selections persist with the DAW session
//! via the CLAP state extension. Loading and rendering happen on a
//! background thread; the audio thread swaps buffers atomically and never
//! blocks. Playback follows the host transport (seconds timeline preferred,
//! beats+tempo fallback, free-run without transport). For VST3-only hosts,
//! wrap the built `.clap` with the free-audio `clap-wrapper` project.

// Implementing the CLAP C ABI requires unsafe throughout; the plugin keeps
// every unsafe block small and auditable.
#![allow(unsafe_code)]

use std::ffi::{c_char, c_void, CStr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    clap_event_param_value, clap_event_transport, clap_input_events, clap_output_events,
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE, CLAP_TRANSPORT_HAS_SECONDS_TIMELINE,
    CLAP_TRANSPORT_HAS_TEMPO, CLAP_TRANSPORT_IS_PLAYING,
};
use clap_sys::ext::gui::{clap_plugin_gui, clap_window, CLAP_EXT_GUI};
use clap_sys::ext::params::{
    clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS, CLAP_PARAM_IS_STEPPED,
};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::fixedpoint::{CLAP_BEATTIME_FACTOR, CLAP_SECTIME_FACTOR};
use clap_sys::host::clap_host;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::{clap_process, clap_process_status, CLAP_PROCESS_CONTINUE};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;
use musicos_render::RenderOptions;

const ID: &CStr = c"org.musicos.player";
const NAME: &CStr = c"MusicOS Player";
const VENDOR: &CStr = c"MusicOS";
const VERSION: &CStr = c"0.2.0";
const EMPTY: &CStr = c"";

/// The single parameter: which library project plays.
const PARAM_PROJECT: u32 = 0;

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

/// Rendered project audio, swapped in atomically by the loader thread.
type Audio = (Vec<f32>, Vec<f32>);

/// Plugin instance state (behind `clap_plugin.plugin_data`).
struct Player {
    /// Discovered `.musicos` bundles, sorted; index = param value.
    library: Vec<PathBuf>,
    /// Currently selected library index.
    selected: u32,
    /// Current audio, owned by the audio thread.
    left: Vec<f32>,
    right: Vec<f32>,
    /// Handoff slot from loader threads (audio thread try-locks only).
    incoming: Arc<Mutex<Option<Audio>>>,
    sample_rate: f64,
    /// Cursor for hosts that provide no transport.
    free_pos: usize,
}

impl Player {
    /// Scans the project library: `MUSICOS_PROJECT` (single override) first,
    /// then every `*.musicos` in `MUSICOS_LIBRARY` or `~/.musicos/projects`.
    fn scan_library() -> Vec<PathBuf> {
        let mut found = Vec::new();
        if let Ok(p) = std::env::var("MUSICOS_PROJECT") {
            if !p.trim().is_empty() {
                found.push(PathBuf::from(p.trim()));
            }
        }
        let dir = std::env::var_os("MUSICOS_LIBRARY").map_or_else(
            || {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(|h| PathBuf::from(h).join(".musicos/projects"))
            },
            |d| Some(PathBuf::from(d)),
        );
        if let Some(dir) = dir {
            if let Ok(entries) = std::fs::read_dir(dir) {
                let mut bundles: Vec<PathBuf> = entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "musicos"))
                    .collect();
                bundles.sort();
                for b in bundles {
                    if !found.contains(&b) {
                        found.push(b);
                    }
                }
            }
        }
        found
    }

    /// Display name of a library entry.
    fn entry_name(&self, index: u32) -> String {
        self.library
            .get(index as usize)
            .and_then(|p| p.file_stem())
            .map_or_else(
                || "(none)".to_string(),
                |s| s.to_string_lossy().into_owned(),
            )
    }

    /// Loads + renders `self.selected` on a background thread; the result
    /// arrives through `incoming` and is swapped in by the audio thread.
    fn request_load(&self) {
        let Some(path) = self.library.get(self.selected as usize).cloned() else {
            eprintln!("MusicOS Player: no project at index {}", self.selected);
            return;
        };
        let slot = Arc::clone(&self.incoming);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let sample_rate = self.sample_rate as u32;
        std::thread::spawn(move || {
            let rendered = musicos_storage::BundleStore::open(&path)
                .map_err(|e| e.to_string())
                .and_then(|store| store.load_state().map_err(|e| e.to_string()))
                .and_then(|state| {
                    let opts = RenderOptions {
                        sample_rate,
                        ..RenderOptions::default()
                    };
                    musicos_render::render_project(&state, &opts).map_err(|e| e.to_string())
                });
            match rendered {
                Ok(buffer) => {
                    *slot.lock().expect("incoming lock") = Some((buffer.left, buffer.right));
                }
                Err(e) => eprintln!("MusicOS Player: cannot load {}: {e}", path.display()),
            }
        });
    }

    /// Applies a "Project" parameter value (from events, flush, or state).
    fn select(&mut self, value: f64) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let index = (value.max(0.0) as u32).min(self.library.len().saturating_sub(1) as u32);
        if index != self.selected || (self.left.is_empty() && !self.library.is_empty()) {
            self.selected = index;
            self.request_load();
        }
    }

    /// Consumes pending param events (shared by `process` and `flush`).
    fn handle_events(&mut self, in_events: *const clap_input_events) {
        if in_events.is_null() {
            return;
        }
        let list = unsafe { &*in_events };
        let (Some(size), Some(get)) = (list.size, list.get) else {
            return;
        };
        for i in 0..unsafe { size(list) } {
            let header = unsafe { get(list, i) };
            if header.is_null() {
                continue;
            }
            let h = unsafe { &*header };
            if h.space_id == CLAP_CORE_EVENT_SPACE_ID && h.type_ == CLAP_EVENT_PARAM_VALUE {
                // Hosts allocate param events with their natural alignment;
                // the header is the event's first field (CLAP ABI).
                #[allow(clippy::cast_ptr_alignment)]
                let ev = unsafe { &*header.cast::<clap_event_param_value>() };
                if ev.param_id == PARAM_PROJECT {
                    self.select(ev.value);
                }
            }
        }
    }

    /// Swaps in freshly loaded audio, if any (audio thread; never blocks).
    fn poll_incoming(&mut self) {
        if let Ok(mut slot) = self.incoming.try_lock() {
            if let Some((l, r)) = slot.take() {
                self.left = l;
                self.right = r;
                self.free_pos = 0;
            }
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

    /// Fills one output block from the current audio.
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

// --- core plugin vtable -----------------------------------------------------

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
    let player = unsafe { &mut *player_of(plugin) };
    player.sample_rate = sample_rate;
    player.library = Player::scan_library();
    player.left.clear();
    player.right.clear();
    player.free_pos = 0;
    if player.library.is_empty() {
        eprintln!(
            "MusicOS Player: no projects found (set MUSICOS_PROJECT or put \
             .musicos bundles in MUSICOS_LIBRARY / ~/.musicos/projects)"
        );
    } else {
        player.request_load();
    }
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
    let player = unsafe { &mut *player_of(plugin) };
    player.poll_incoming();
    player.handle_events(process.in_events);
    let frames = process.frames_count as usize;
    if process.audio_outputs_count >= 1 {
        let output = unsafe { &*process.audio_outputs };
        if output.channel_count >= 2 {
            let l = unsafe { std::slice::from_raw_parts_mut(*output.data32, frames) };
            let r = unsafe { std::slice::from_raw_parts_mut(*output.data32.add(1), frames) };
            player.fill(process.transport, l, r);
        }
    }
    CLAP_PROCESS_CONTINUE
}

// --- params extension ---------------------------------------------------------

fn write_cstr(dst: &mut [c_char], text: &str) {
    let bytes = text.as_bytes();
    let n = bytes.len().min(dst.len().saturating_sub(1));
    for (i, b) in bytes[..n].iter().enumerate() {
        #[allow(clippy::cast_possible_wrap)]
        {
            dst[i] = *b as c_char;
        }
    }
    dst[n] = 0;
}

unsafe extern "C" fn params_count(_plugin: *const clap_plugin) -> u32 {
    1
}

unsafe extern "C" fn params_get_info(
    plugin: *const clap_plugin,
    param_index: u32,
    param_info: *mut clap_param_info,
) -> bool {
    if param_index != 0 || param_info.is_null() {
        return false;
    }
    let player = unsafe { &*player_of(plugin) };
    let info = unsafe { &mut *param_info };
    info.id = PARAM_PROJECT;
    info.flags = CLAP_PARAM_IS_STEPPED;
    info.cookie = std::ptr::null_mut();
    write_cstr(&mut info.name, "Project");
    write_cstr(&mut info.module, "");
    info.min_value = 0.0;
    info.max_value = player.library.len().saturating_sub(1) as f64;
    info.default_value = 0.0;
    true
}

unsafe extern "C" fn params_get_value(
    plugin: *const clap_plugin,
    param_id: u32,
    out_value: *mut f64,
) -> bool {
    if param_id != PARAM_PROJECT || out_value.is_null() {
        return false;
    }
    unsafe { *out_value = f64::from((*player_of(plugin)).selected) };
    true
}

unsafe extern "C" fn params_value_to_text(
    plugin: *const clap_plugin,
    param_id: u32,
    value: f64,
    out_buffer: *mut c_char,
    out_buffer_capacity: u32,
) -> bool {
    if param_id != PARAM_PROJECT || out_buffer.is_null() || out_buffer_capacity == 0 {
        return false;
    }
    let player = unsafe { &*player_of(plugin) };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let name = player.entry_name(value.max(0.0) as u32);
    let dst = unsafe { std::slice::from_raw_parts_mut(out_buffer, out_buffer_capacity as usize) };
    write_cstr(dst, &name);
    true
}

unsafe extern "C" fn params_text_to_value(
    plugin: *const clap_plugin,
    param_id: u32,
    param_value_text: *const c_char,
    out_value: *mut f64,
) -> bool {
    if param_id != PARAM_PROJECT || param_value_text.is_null() || out_value.is_null() {
        return false;
    }
    let text = unsafe { CStr::from_ptr(param_value_text) }.to_string_lossy();
    let player = unsafe { &*player_of(plugin) };
    if let Ok(n) = text.trim().parse::<u32>() {
        unsafe { *out_value = f64::from(n) };
        return true;
    }
    for i in 0..player.library.len() as u32 {
        if player.entry_name(i) == text.trim() {
            unsafe { *out_value = f64::from(i) };
            return true;
        }
    }
    false
}

unsafe extern "C" fn params_flush(
    plugin: *const clap_plugin,
    in_: *const clap_input_events,
    _out: *const clap_output_events,
) {
    unsafe { &mut *player_of(plugin) }.handle_events(in_);
}

static PARAMS_VTABLE: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_get_info),
    get_value: Some(params_get_value),
    value_to_text: Some(params_value_to_text),
    text_to_value: Some(params_text_to_value),
    flush: Some(params_flush),
};

// --- state extension ----------------------------------------------------------

unsafe extern "C" fn state_save(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool {
    if stream.is_null() {
        return false;
    }
    let player = unsafe { &*player_of(plugin) };
    let Some(path) = player.library.get(player.selected as usize) else {
        return true; // nothing selected: empty state
    };
    let bytes = path.to_string_lossy().into_owned().into_bytes();
    let Some(write) = (unsafe { (*stream).write }) else {
        return false;
    };
    let mut off = 0usize;
    while off < bytes.len() {
        let n = unsafe {
            write(
                stream,
                bytes[off..].as_ptr().cast(),
                (bytes.len() - off) as u64,
            )
        };
        if n <= 0 {
            return false;
        }
        #[allow(clippy::cast_sign_loss)]
        {
            off += n as usize;
        }
    }
    true
}

unsafe extern "C" fn state_load(plugin: *const clap_plugin, stream: *const clap_istream) -> bool {
    if stream.is_null() {
        return false;
    }
    let Some(read) = (unsafe { (*stream).read }) else {
        return false;
    };
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        let n = unsafe { read(stream, chunk.as_mut_ptr().cast(), chunk.len() as u64) };
        if n < 0 {
            return false;
        }
        if n == 0 {
            break;
        }
        #[allow(clippy::cast_sign_loss)]
        bytes.extend_from_slice(&chunk[..n as usize]);
    }
    let player = unsafe { &mut *player_of(plugin) };
    if bytes.is_empty() {
        return true;
    }
    let path = PathBuf::from(String::from_utf8_lossy(&bytes).into_owned());
    let index = player
        .library
        .iter()
        .position(|p| *p == path)
        .unwrap_or_else(|| {
            player.library.push(path);
            player.library.len() - 1
        });
    player.selected = u32::MAX; // force reload even if index matches
    player.select(index as f64);
    true
}

static STATE_VTABLE: clap_plugin_state = clap_plugin_state {
    save: Some(state_save),
    load: Some(state_load),
};

unsafe extern "C" fn plugin_get_extension(
    _plugin: *const clap_plugin,
    id: *const c_char,
) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_PARAMS {
        (&raw const PARAMS_VTABLE).cast()
    } else if id == CLAP_EXT_STATE {
        (&raw const STATE_VTABLE).cast()
    } else if id == CLAP_EXT_GUI {
        (&raw const GUI_VTABLE).cast()
    } else {
        std::ptr::null()
    }
}

unsafe extern "C" fn plugin_on_main_thread(_plugin: *const clap_plugin) {}

// --- gui extension --------------------------------------------------------------
//
// v1 GUI: a floating native project-picker. `show()` opens the platform's
// file dialog (main thread, as CLAP guarantees for gui calls); picking a
// `.musicos` bundle adds it to the library, selects it, and re-renders in
// the background. A full custom panel can replace this without changing
// the host-facing surface.

unsafe extern "C" fn gui_is_api_supported(
    _plugin: *const clap_plugin,
    _api: *const c_char,
    is_floating: bool,
) -> bool {
    is_floating
}

unsafe extern "C" fn gui_get_preferred_api(
    _plugin: *const clap_plugin,
    _api: *mut *const c_char,
    is_floating: *mut bool,
) -> bool {
    if !is_floating.is_null() {
        unsafe { *is_floating = true };
    }
    true
}

unsafe extern "C" fn gui_create(
    _plugin: *const clap_plugin,
    _api: *const c_char,
    is_floating: bool,
) -> bool {
    is_floating
}

unsafe extern "C" fn gui_destroy(_plugin: *const clap_plugin) {}

unsafe extern "C" fn gui_set_scale(_plugin: *const clap_plugin, _scale: f64) -> bool {
    true
}

unsafe extern "C" fn gui_get_size(
    _plugin: *const clap_plugin,
    width: *mut u32,
    height: *mut u32,
) -> bool {
    if !width.is_null() {
        unsafe { *width = 0 };
    }
    if !height.is_null() {
        unsafe { *height = 0 };
    }
    true
}

unsafe extern "C" fn gui_can_resize(_plugin: *const clap_plugin) -> bool {
    false
}

unsafe extern "C" fn gui_set_parent(
    _plugin: *const clap_plugin,
    _window: *const clap_window,
) -> bool {
    false // floating only
}

unsafe extern "C" fn gui_set_transient(
    _plugin: *const clap_plugin,
    _window: *const clap_window,
) -> bool {
    true
}

unsafe extern "C" fn gui_suggest_title(_plugin: *const clap_plugin, _title: *const c_char) {}

unsafe extern "C" fn gui_show(plugin: *const clap_plugin) -> bool {
    let player = unsafe { &mut *player_of(plugin) };
    let mut dialog = rfd::FileDialog::new().set_title("Choose a MusicOS project (.musicos)");
    if let Some(first) = player.library.first().and_then(|p| p.parent()) {
        dialog = dialog.set_directory(first);
    }
    let Some(path) = dialog.pick_folder() else {
        return true; // canceled: nothing to do, still "shown"
    };
    if path.extension() != Some(std::ffi::OsStr::new("musicos")) {
        eprintln!(
            "MusicOS Player: {} is not a .musicos bundle",
            path.display()
        );
        return true;
    }
    let index = player
        .library
        .iter()
        .position(|p| *p == path)
        .unwrap_or_else(|| {
            player.library.push(path);
            player.library.len() - 1
        });
    player.selected = u32::MAX; // force reload even if index matches
    player.select(index as f64);
    true
}

unsafe extern "C" fn gui_hide(_plugin: *const clap_plugin) -> bool {
    true
}

static GUI_VTABLE: clap_plugin_gui = clap_plugin_gui {
    is_api_supported: Some(gui_is_api_supported),
    get_preferred_api: Some(gui_get_preferred_api),
    create: Some(gui_create),
    destroy: Some(gui_destroy),
    set_scale: Some(gui_set_scale),
    get_size: Some(gui_get_size),
    can_resize: Some(gui_can_resize),
    get_resize_hints: None,
    adjust_size: None,
    set_size: None,
    set_parent: Some(gui_set_parent),
    set_transient: Some(gui_set_transient),
    suggest_title: Some(gui_suggest_title),
    show: Some(gui_show),
    hide: Some(gui_hide),
};

// --- factory + entry ----------------------------------------------------------

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
        library: Vec::new(),
        selected: 0,
        left: Vec::new(),
        right: Vec::new(),
        incoming: Arc::new(Mutex::new(None)),
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
