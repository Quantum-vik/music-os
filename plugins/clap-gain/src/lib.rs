//! A minimal but complete CLAP plugin: a fixed −6 dB stereo gain.
//!
//! This crate exists to give the MusicOS CLAP host (`musicos-plugin-host`) a
//! real dynamic library to load in tests and CI — the exported [`clap_entry`]
//! goes through the same dlopen → entry → factory → instance path any
//! third-party `.clap` does. It is intentionally tiny: one plugin, no
//! extensions, no events.

use std::ffi::{c_char, c_void, CStr};

use clap_sys::entry::clap_plugin_entry;
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::process::{clap_process, clap_process_status, CLAP_PROCESS_CONTINUE};
use clap_sys::version::CLAP_VERSION;

/// Gain applied by the plugin (−6 dB), asserted by host tests.
pub const GAIN: f32 = 0.5;

const ID: &CStr = c"org.musicos.test-gain";
const NAME: &CStr = c"MusicOS Test Gain";
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
    SyncWrap([c"audio-effect".as_ptr(), std::ptr::null()]);

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

unsafe extern "C" fn plugin_init(_plugin: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn plugin_destroy(plugin: *const clap_plugin) {
    if !plugin.is_null() {
        drop(unsafe { Box::from_raw(plugin.cast_mut()) });
    }
}

unsafe extern "C" fn plugin_activate(
    _plugin: *const clap_plugin,
    _sample_rate: f64,
    _min_frames: u32,
    _max_frames: u32,
) -> bool {
    true
}

unsafe extern "C" fn plugin_deactivate(_plugin: *const clap_plugin) {}

unsafe extern "C" fn plugin_start_processing(_plugin: *const clap_plugin) -> bool {
    true
}

unsafe extern "C" fn plugin_stop_processing(_plugin: *const clap_plugin) {}

unsafe extern "C" fn plugin_reset(_plugin: *const clap_plugin) {}

unsafe extern "C" fn plugin_process(
    _plugin: *const clap_plugin,
    process: *const clap_process,
) -> clap_process_status {
    let process = unsafe { &*process };
    let frames = process.frames_count as usize;
    if process.audio_inputs_count >= 1 && process.audio_outputs_count >= 1 {
        let input = unsafe { &*process.audio_inputs };
        let output = unsafe { &*process.audio_outputs };
        let channels = input.channel_count.min(output.channel_count) as usize;
        for ch in 0..channels {
            let src = unsafe { *input.data32.add(ch) };
            let dst = unsafe { *output.data32.add(ch) };
            for i in 0..frames {
                unsafe { *dst.add(i) = *src.add(i) * GAIN };
            }
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
    Box::into_raw(Box::new(clap_plugin {
        desc: &raw const DESCRIPTOR.0,
        plugin_data: std::ptr::null_mut(),
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
#[allow(non_upper_case_globals, unsafe_code)]
#[no_mangle]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_get_factory),
};
