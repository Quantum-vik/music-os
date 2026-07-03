//! Loading and hosting CLAP plugins (dlopen + `clap_entry`).
//!
//! [`ClapLibrary::load`] opens a `.clap` dynamic library, resolves the
//! exported `clap_entry`, initializes it, and exposes the plugin factory.
//! [`ClapInstance`] adapts one created plugin to the MusicOS
//! [`ProcessorPlugin`] contract so hosted CLAP effects slot into the same
//! insert chains as native plugins (docs/09 §4).
//!
//! Scope of this milestone: stereo audio effects, no events, no extensions —
//! parameters and instruments arrive with the events milestone.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{clap_event_header, clap_input_events, clap_output_events};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::clap_process;
use clap_sys::version::CLAP_VERSION;
use musicos_plugin_api::{PluginDescriptor, PluginError, PluginKind, ProcessorPlugin};

/// Errors from loading or driving a CLAP library.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ClapHostError {
    /// The dynamic library could not be opened.
    #[error("failed to load library: {0}")]
    Load(String),
    /// The library does not export `clap_entry`.
    #[error("no clap_entry symbol: {0}")]
    NoEntry(String),
    /// `clap_entry.init` returned false or a vtable slot was missing.
    #[error("plugin refused: {0}")]
    Refused(&'static str),
    /// The requested plugin id is not provided by this library.
    #[error("plugin id not found: {0}")]
    UnknownId(String),
}

/// Identity of one plugin inside a CLAP library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClapPluginInfo {
    /// Reverse-DNS plugin id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Vendor string.
    pub vendor: String,
    /// Version string.
    pub version: String,
}

// The host structure handed to plugins. MusicOS offers no extensions yet, so
// every callback is a safe no-op.
unsafe extern "C" fn host_get_extension(
    _host: *const clap_host,
    _id: *const c_char,
) -> *const c_void {
    std::ptr::null()
}
unsafe extern "C" fn host_request_restart(_host: *const clap_host) {}
unsafe extern "C" fn host_request_process(_host: *const clap_host) {}
unsafe extern "C" fn host_request_callback(_host: *const clap_host) {}

const HOST_NAME: &CStr = c"MusicOS";
const HOST_URL: &CStr = c"https://github.com/Quantum-vik/music-os";
const HOST_VERSION: &CStr = c"0.1.0";

/// Wrapper making pointer-holding CLAP statics `Sync`; the pointed-to data
/// is immutable `'static`, so cross-thread reads are safe.
#[repr(transparent)]
struct SyncWrap<T>(T);
// SAFETY: only wraps immutable static data.
unsafe impl<T> Sync for SyncWrap<T> {}

static HOST: SyncWrap<clap_host> = SyncWrap(clap_host {
    clap_version: CLAP_VERSION,
    host_data: std::ptr::null_mut(),
    name: HOST_NAME.as_ptr(),
    vendor: HOST_NAME.as_ptr(),
    url: HOST_URL.as_ptr(),
    version: HOST_VERSION.as_ptr(),
    get_extension: Some(host_get_extension),
    request_restart: Some(host_request_restart),
    request_process: Some(host_request_process),
    request_callback: Some(host_request_callback),
});

// Empty event queues for the no-events milestone.
unsafe extern "C" fn in_events_size(_list: *const clap_input_events) -> u32 {
    0
}
unsafe extern "C" fn in_events_get(
    _list: *const clap_input_events,
    _index: u32,
) -> *const clap_event_header {
    std::ptr::null()
}
unsafe extern "C" fn out_events_try_push(
    _list: *const clap_output_events,
    _event: *const clap_event_header,
) -> bool {
    false
}

static IN_EVENTS: SyncWrap<clap_input_events> = SyncWrap(clap_input_events {
    ctx: std::ptr::null_mut(),
    size: Some(in_events_size),
    get: Some(in_events_get),
});
static OUT_EVENTS: SyncWrap<clap_output_events> = SyncWrap(clap_output_events {
    ctx: std::ptr::null_mut(),
    try_push: Some(out_events_try_push),
});

/// An opened CLAP dynamic library with an initialized `clap_entry`.
pub struct ClapLibrary {
    // Field order matters: instances borrow `entry` conceptually, and the
    // library must be dropped (dlclose) last.
    entry: *const clap_plugin_entry,
    _lib: libloading::Library,
}

// SAFETY: the entry pointer targets a static inside the loaded library, which
// lives as long as `_lib`; CLAP entry/factory calls are main-thread-safe.
unsafe impl Send for ClapLibrary {}

impl ClapLibrary {
    /// Opens a `.clap` library, resolves `clap_entry`, and calls `init`.
    ///
    /// # Errors
    /// Fails if the library cannot be opened, exports no `clap_entry`, or
    /// its `init` refuses the load.
    ///
    /// # Safety
    /// Loading a plugin runs arbitrary code from that library — only load
    /// binaries the user chose to install (docs/09 §3 quarantine applies
    /// before this call, not inside it).
    pub unsafe fn load(path: &Path) -> Result<ClapLibrary, ClapHostError> {
        let lib = unsafe { libloading::Library::new(path) }
            .map_err(|e| ClapHostError::Load(e.to_string()))?;
        let entry: *const clap_plugin_entry = unsafe {
            lib.get::<*const clap_plugin_entry>(b"clap_entry\0")
                .map(|sym| {
                    // The symbol IS the static struct, not a pointer to it.
                    std::ptr::from_ref::<clap_plugin_entry>(&**sym)
                })
                .map_err(|e| ClapHostError::NoEntry(e.to_string()))?
        };
        let init = unsafe { (*entry).init }.ok_or(ClapHostError::Refused("entry.init missing"))?;
        let c_path = CString::new(path.to_string_lossy().as_bytes())
            .map_err(|_| ClapHostError::Refused("path contains NUL"))?;
        if !unsafe { init(c_path.as_ptr()) } {
            return Err(ClapHostError::Refused("entry.init returned false"));
        }
        Ok(ClapLibrary { entry, _lib: lib })
    }

    fn factory(&self) -> Result<*const clap_plugin_factory, ClapHostError> {
        let get_factory = unsafe { (*self.entry).get_factory }
            .ok_or(ClapHostError::Refused("entry.get_factory missing"))?;
        let factory =
            unsafe { get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) }.cast::<clap_plugin_factory>();
        if factory.is_null() {
            return Err(ClapHostError::Refused("no plugin factory"));
        }
        Ok(factory)
    }

    /// Lists every plugin the library provides.
    ///
    /// # Errors
    /// Fails if the library exposes no plugin factory.
    pub fn plugins(&self) -> Result<Vec<ClapPluginInfo>, ClapHostError> {
        let factory = self.factory()?;
        let count_fn = unsafe { (*factory).get_plugin_count }
            .ok_or(ClapHostError::Refused("factory.get_plugin_count missing"))?;
        let desc_fn = unsafe { (*factory).get_plugin_descriptor }.ok_or(ClapHostError::Refused(
            "factory.get_plugin_descriptor missing",
        ))?;
        let count = unsafe { count_fn(factory) };
        let mut infos = Vec::new();
        for index in 0..count {
            let desc = unsafe { desc_fn(factory, index) };
            if desc.is_null() {
                continue;
            }
            let text = |ptr: *const c_char| -> String {
                if ptr.is_null() {
                    String::new()
                } else {
                    unsafe { CStr::from_ptr(ptr) }
                        .to_string_lossy()
                        .into_owned()
                }
            };
            infos.push(ClapPluginInfo {
                id: text(unsafe { (*desc).id }),
                name: text(unsafe { (*desc).name }),
                vendor: text(unsafe { (*desc).vendor }),
                version: text(unsafe { (*desc).version }),
            });
        }
        Ok(infos)
    }

    /// Creates and initializes a plugin instance by id.
    ///
    /// # Errors
    /// Fails if the id is unknown or the plugin refuses creation/init.
    pub fn instantiate(&self, id: &str) -> Result<ClapInstance, ClapHostError> {
        let info = self
            .plugins()?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| ClapHostError::UnknownId(id.to_string()))?;
        let factory = self.factory()?;
        let create = unsafe { (*factory).create_plugin }
            .ok_or(ClapHostError::Refused("factory.create_plugin missing"))?;
        let c_id = CString::new(id).map_err(|_| ClapHostError::Refused("id contains NUL"))?;
        let plugin = unsafe { create(factory, &raw const HOST.0, c_id.as_ptr()) };
        if plugin.is_null() {
            return Err(ClapHostError::Refused("create_plugin returned null"));
        }
        let init =
            unsafe { (*plugin).init }.ok_or(ClapHostError::Refused("plugin.init missing"))?;
        if !unsafe { init(plugin) } {
            unsafe {
                if let Some(destroy) = (*plugin).destroy {
                    destroy(plugin);
                }
            }
            return Err(ClapHostError::Refused("plugin.init returned false"));
        }
        Ok(ClapInstance {
            plugin,
            info,
            active: false,
            scratch_l: Vec::new(),
            scratch_r: Vec::new(),
        })
    }
}

/// One live CLAP plugin instance, adapted to [`ProcessorPlugin`].
pub struct ClapInstance {
    plugin: *const clap_plugin,
    info: ClapPluginInfo,
    active: bool,
    scratch_l: Vec<f32>,
    scratch_r: Vec<f32>,
}

// SAFETY: MusicOS drives each instance from one thread at a time; the raw
// pointer is owned by this struct and destroyed on drop.
unsafe impl Send for ClapInstance {}

impl ClapInstance {
    /// The plugin's identity as reported by its descriptor.
    pub fn info(&self) -> &ClapPluginInfo {
        &self.info
    }

    fn deactivate(&mut self) {
        if !self.active {
            return;
        }
        unsafe {
            if let Some(stop) = (*self.plugin).stop_processing {
                stop(self.plugin);
            }
            if let Some(deactivate) = (*self.plugin).deactivate {
                deactivate(self.plugin);
            }
        }
        self.active = false;
    }
}

impl ProcessorPlugin for ClapInstance {
    fn descriptor(&self) -> PluginDescriptor {
        // ProcessorPlugin descriptors are &'static (native plugins embed
        // them); loaded CLAP strings live as long as the process, so leaking
        // one small copy per loaded plugin is the honest equivalent.
        PluginDescriptor {
            id: Box::leak(self.info.id.clone().into_boxed_str()),
            name: Box::leak(self.info.name.clone().into_boxed_str()),
            vendor: Box::leak(self.info.vendor.clone().into_boxed_str()),
            version: Box::leak(self.info.version.clone().into_boxed_str()),
            kind: PluginKind::Effect,
        }
    }

    fn set_param(&mut self, id: &str, _value: f32) -> Result<(), PluginError> {
        // Parameter surfacing arrives with the CLAP params-extension
        // milestone; until then every id is unknown.
        Err(PluginError::UnknownParam(id.to_string()))
    }

    fn prepare(&mut self, sample_rate: u32, max_block: usize) {
        self.deactivate();
        self.scratch_l.resize(max_block, 0.0);
        self.scratch_r.resize(max_block, 0.0);
        unsafe {
            if let Some(activate) = (*self.plugin).activate {
                if activate(self.plugin, f64::from(sample_rate), 1, max_block as u32) {
                    if let Some(start) = (*self.plugin).start_processing {
                        if start(self.plugin) {
                            self.active = true;
                        }
                    }
                }
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let frames = left.len().min(right.len());
        if !self.active || frames == 0 || frames > self.scratch_l.len() {
            return;
        }
        self.scratch_l[..frames].copy_from_slice(&left[..frames]);
        self.scratch_r[..frames].copy_from_slice(&right[..frames]);

        let mut in_channels = [self.scratch_l.as_mut_ptr(), self.scratch_r.as_mut_ptr()];
        let mut out_channels = [left.as_mut_ptr(), right.as_mut_ptr()];
        let input = clap_audio_buffer {
            data32: in_channels.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let mut output = clap_audio_buffer {
            data32: out_channels.as_mut_ptr(),
            data64: std::ptr::null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let process = clap_process {
            steady_time: -1,
            frames_count: frames as u32,
            transport: std::ptr::null(),
            audio_inputs: &raw const input,
            audio_outputs: &raw mut output,
            audio_inputs_count: 1,
            audio_outputs_count: 1,
            in_events: &raw const IN_EVENTS.0,
            out_events: &raw const OUT_EVENTS.0,
        };
        unsafe {
            if let Some(process_fn) = (*self.plugin).process {
                process_fn(self.plugin, &raw const process);
            }
        }
    }

    fn reset(&mut self) {
        unsafe {
            if let Some(reset) = (*self.plugin).reset {
                reset(self.plugin);
            }
        }
    }
}

impl Drop for ClapInstance {
    fn drop(&mut self) {
        self.deactivate();
        unsafe {
            if let Some(destroy) = (*self.plugin).destroy {
                destroy(self.plugin);
            }
        }
    }
}
