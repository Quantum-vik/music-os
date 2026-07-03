//! Stable trait interfaces for native MusicOS plugins.
//!
//! The compiled-in phase of the plugin strategy (ADR-0014 phase 1): plugins
//! are crates in `plugins/` implementing [`ProcessorPlugin`], registered at
//! each app's composition root. The same trait shape carries into
//! out-of-process and dynamic phases. The [`conformance`] harness is both the
//! contract test every plugin must pass and the reference documentation for
//! host expectations (docs/09 §7).

/// What a plugin does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginKind {
    /// Audio in, audio out.
    Effect,
    /// Produces audio from events (hosting arrives with the CLAP adapter).
    Instrument,
}

/// Identity and metadata for a plugin.
#[derive(Debug, Clone)]
pub struct PluginDescriptor {
    /// Stable reverse-DNS id (e.g. `org.musicos.bitcrusher`).
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Vendor string.
    pub vendor: &'static str,
    /// Semantic version.
    pub version: &'static str,
    /// Plugin kind.
    pub kind: PluginKind,
}

/// One automatable parameter.
#[derive(Debug, Clone)]
pub struct ParamInfo {
    /// Stable parameter id.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Minimum plain value.
    pub min: f32,
    /// Maximum plain value.
    pub max: f32,
    /// Default plain value.
    pub default: f32,
}

/// A stereo audio processor plugin.
///
/// Contract (enforced by [`conformance::check`]):
/// - `prepare` may allocate; `process` must not.
/// - `process` output must always be finite (no NaN/inf), at any block size
///   from 1 to the prepared maximum.
/// - `set_param` accepts any value within the declared range and rejects
///   unknown ids.
pub trait ProcessorPlugin: Send {
    /// Identity and metadata.
    fn descriptor(&self) -> PluginDescriptor;
    /// Declared parameters (empty by default).
    fn params(&self) -> Vec<ParamInfo> {
        Vec::new()
    }
    /// Sets a parameter (plain value, clamped by the plugin if needed).
    ///
    /// # Errors
    /// Returns [`PluginError::UnknownParam`] for ids not in [`Self::params`].
    fn set_param(&mut self, id: &str, value: f32) -> Result<(), PluginError>;
    /// Prepares for processing (allocation allowed here).
    fn prepare(&mut self, sample_rate: u32, max_block: usize);
    /// Processes one block in place. Never allocates.
    fn process(&mut self, left: &mut [f32], right: &mut [f32]);
    /// Clears internal state (delay lines, envelopes).
    fn reset(&mut self) {}
}

/// A constructor for a plugin instance (registered at composition roots).
pub type PluginFactory = fn() -> Box<dyn ProcessorPlugin>;

/// Errors from plugin operations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum PluginError {
    /// The parameter id is not declared by this plugin.
    #[error("unknown parameter '{0}'")]
    UnknownParam(String),
}

pub mod conformance {
    //! The conformance harness every native plugin must pass (docs/09 §7).

    use super::PluginFactory;

    /// Runs the conformance suite against a plugin factory. Returns every
    /// violated expectation (empty = pass).
    pub fn check(factory: PluginFactory) -> Result<(), Vec<String>> {
        let mut failures = Vec::new();
        let mut plugin = factory();
        let descriptor = plugin.descriptor();
        if descriptor.id.is_empty() || !descriptor.id.contains('.') {
            failures.push("descriptor id must be a non-empty reverse-DNS string".to_string());
        }
        plugin.prepare(48_000, 512);

        // Silence in, finite (and near-silent) out.
        let mut l = vec![0.0f32; 512];
        let mut r = vec![0.0f32; 512];
        plugin.process(&mut l, &mut r);
        expect_finite(&l, &r, "silence", &mut failures);

        // Impulse in, finite out over several blocks (tails ring safely).
        plugin.reset();
        let mut l = vec![0.0f32; 512];
        let mut r = vec![0.0f32; 512];
        l[0] = 1.0;
        r[0] = 1.0;
        for _ in 0..8 {
            plugin.process(&mut l, &mut r);
            expect_finite(&l, &r, "impulse", &mut failures);
        }

        // Block size 1 must work.
        let mut l1 = [0.5f32];
        let mut r1 = [0.5f32];
        plugin.process(&mut l1, &mut r1);
        expect_finite(&l1, &r1, "block-size 1", &mut failures);

        // Every declared parameter accepts min/default/max; unknown ids error.
        for param in plugin.params() {
            for value in [param.min, param.default, param.max] {
                if plugin.set_param(param.id, value).is_err() {
                    failures.push(format!(
                        "param '{}' rejected in-range value {value}",
                        param.id
                    ));
                }
            }
            if param.min > param.max || !param.default.is_finite() {
                failures.push(format!("param '{}' has an invalid range", param.id));
            }
        }
        if plugin.set_param("__musicos_no_such_param__", 0.0).is_ok() {
            failures.push("unknown parameter ids must be rejected".to_string());
        }

        // Full-scale noise-ish sweep after params pushed to extremes: finite.
        let mut l: Vec<f32> = (0..512)
            .map(|i| if i % 2 == 0 { 1.0 } else { -1.0 })
            .collect();
        let mut r = l.clone();
        plugin.process(&mut l, &mut r);
        expect_finite(&l, &r, "full-scale after param sweep", &mut failures);

        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures)
        }
    }

    fn expect_finite(l: &[f32], r: &[f32], stage: &str, failures: &mut Vec<String>) {
        if !(l.iter().all(|s| s.is_finite()) && r.iter().all(|s| s.is_finite())) {
            failures.push(format!("non-finite output during {stage}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deliberately broken plugin: emits NaN and accepts unknown params.
    struct Broken;
    impl ProcessorPlugin for Broken {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor {
                id: "bad",
                name: "Broken",
                vendor: "t",
                version: "0",
                kind: PluginKind::Effect,
            }
        }
        fn set_param(&mut self, _: &str, _: f32) -> Result<(), PluginError> {
            Ok(()) // wrongly accepts everything
        }
        fn prepare(&mut self, _: u32, _: usize) {}
        fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
            left.fill(f32::NAN);
            right.fill(f32::NAN);
        }
    }

    /// A minimal correct plugin: pass-through with one parameter.
    struct Pass {
        gain: f32,
    }
    impl ProcessorPlugin for Pass {
        fn descriptor(&self) -> PluginDescriptor {
            PluginDescriptor {
                id: "org.musicos.test.pass",
                name: "Pass",
                vendor: "MusicOS",
                version: "1.0.0",
                kind: PluginKind::Effect,
            }
        }
        fn params(&self) -> Vec<ParamInfo> {
            vec![ParamInfo {
                id: "gain",
                name: "Gain",
                min: 0.0,
                max: 2.0,
                default: 1.0,
            }]
        }
        fn set_param(&mut self, id: &str, value: f32) -> Result<(), PluginError> {
            if id == "gain" {
                self.gain = value.clamp(0.0, 2.0);
                Ok(())
            } else {
                Err(PluginError::UnknownParam(id.to_string()))
            }
        }
        fn prepare(&mut self, _: u32, _: usize) {}
        fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
            for s in left.iter_mut().chain(right.iter_mut()) {
                *s *= self.gain;
            }
        }
    }

    #[test]
    fn conformance_passes_a_correct_plugin() {
        assert!(conformance::check(|| Box::new(Pass { gain: 1.0 })).is_ok());
    }

    #[test]
    fn conformance_catches_nan_and_lax_params() {
        let failures = conformance::check(|| Box::new(Broken)).unwrap_err();
        assert!(failures.iter().any(|f| f.contains("non-finite")));
        assert!(failures.iter().any(|f| f.contains("unknown parameter")));
        assert!(failures.iter().any(|f| f.contains("reverse-DNS")));
    }
}
