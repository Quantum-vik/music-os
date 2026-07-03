//! Plugin discovery, lifecycle management, and hosted plugin adapters.
//!
//! Phase 5 groundwork (docs/09): [`HostRegistry`] holds compiled-in native
//! plugins (registered at each app's composition root — ADR-0014 phase 1),
//! and [`discover_clap`] scans the platform's standard CLAP directories for
//! installed bundles. Loading CLAP binaries (dlopen + `clap_entry`) is the
//! next milestone; discovery is deliberately separate so scan results can be
//! cached and quarantined per docs/09 §3.

use std::path::PathBuf;

use musicos_plugin_api::{PluginDescriptor, PluginFactory, ProcessorPlugin};

/// Registry of compiled-in native plugins.
#[derive(Default)]
pub struct HostRegistry {
    factories: Vec<PluginFactory>,
}

impl HostRegistry {
    /// An empty registry.
    pub fn new() -> HostRegistry {
        HostRegistry::default()
    }

    /// A registry preloaded with the first-party plugin set.
    pub fn with_builtins() -> HostRegistry {
        let mut registry = HostRegistry::new();
        registry.register(musicos_plugin_bitcrusher::Bitcrusher::factory);
        registry
    }

    /// Registers a plugin factory.
    pub fn register(&mut self, factory: PluginFactory) {
        self.factories.push(factory);
    }

    /// Descriptors of every registered plugin.
    pub fn descriptors(&self) -> Vec<PluginDescriptor> {
        self.factories.iter().map(|f| f().descriptor()).collect()
    }

    /// Instantiates a plugin by its reverse-DNS id.
    pub fn instantiate(&self, id: &str) -> Option<Box<dyn ProcessorPlugin>> {
        self.factories
            .iter()
            .map(|f| f())
            .find(|p| p.descriptor().id == id)
    }
}

/// A CLAP bundle found on disk (not yet loaded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClapCandidate {
    /// Path to the `.clap` bundle/library.
    pub path: PathBuf,
}

/// The platform's standard CLAP search paths (plus `$CLAP_PATH` entries).
pub fn clap_search_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(extra) = std::env::var("CLAP_PATH") {
        paths.extend(std::env::split_paths(&extra));
    }
    if let Ok(home) = std::env::var("HOME") {
        let home = PathBuf::from(home);
        if cfg!(target_os = "macos") {
            paths.push(home.join("Library/Audio/Plug-Ins/CLAP"));
        } else {
            paths.push(home.join(".clap"));
        }
    }
    if cfg!(target_os = "macos") {
        paths.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));
    } else if cfg!(windows) {
        if let Ok(common) = std::env::var("COMMONPROGRAMFILES") {
            paths.push(PathBuf::from(common).join("CLAP"));
        }
    } else {
        paths.push(PathBuf::from("/usr/lib/clap"));
    }
    paths
}

/// Scans the standard paths for `.clap` bundles.
pub fn discover_clap() -> Vec<ClapCandidate> {
    clap_search_paths()
        .iter()
        .flat_map(|p| scan_dir(p))
        .collect()
}

/// Scans one directory (non-recursive except macOS bundle dirs).
pub fn scan_dir(dir: &std::path::Path) -> Vec<ClapCandidate> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "clap"))
        .map(|path| ClapCandidate { path })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicos_plugin_bitcrusher::Bitcrusher;

    #[test]
    fn registry_lists_and_instantiates() {
        let mut registry = HostRegistry::new();
        registry.register(Bitcrusher::factory);
        let descriptors = registry.descriptors();
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].id, "org.musicos.bitcrusher");
        let mut instance = registry.instantiate("org.musicos.bitcrusher").unwrap();
        instance.prepare(48_000, 512);
        assert!(registry.instantiate("org.musicos.nope").is_none());
    }

    #[test]
    fn scan_finds_clap_bundles() {
        let dir = std::env::temp_dir().join(format!("musicos-clap-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Cool Synth.clap")).unwrap();
        std::fs::write(dir.join("notes.txt"), "not a plugin").unwrap();
        let found = scan_dir(&dir);
        assert_eq!(found.len(), 1);
        assert!(found[0].path.ends_with("Cool Synth.clap"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
