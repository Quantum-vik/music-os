//! First-party bitcrusher effect plugin.
//!
//! The reference native plugin (ADR-0014 phase 1: compiled-in): bit-depth
//! quantization plus sample-and-hold rate reduction. Exists to prove the
//! `plugin-api` contract end to end — it must always pass the conformance
//! harness (docs/09 §7).

use musicos_plugin_api::{ParamInfo, PluginDescriptor, PluginError, PluginKind, ProcessorPlugin};

/// Bit-depth + sample-rate reduction.
#[derive(Debug, Clone)]
pub struct Bitcrusher {
    bits: f32,
    downsample: f32,
    hold: [f32; 2],
    counter: f32,
}

impl Bitcrusher {
    /// A crusher at its default settings (12 bits, no downsampling).
    pub fn new() -> Bitcrusher {
        Bitcrusher {
            bits: 12.0,
            downsample: 1.0,
            hold: [0.0; 2],
            counter: 0.0,
        }
    }

    /// Factory for host registries.
    pub fn factory() -> Box<dyn ProcessorPlugin> {
        Box::new(Bitcrusher::new())
    }
}

impl Default for Bitcrusher {
    fn default() -> Self {
        Bitcrusher::new()
    }
}

impl ProcessorPlugin for Bitcrusher {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: "org.musicos.bitcrusher",
            name: "Bitcrusher",
            vendor: "MusicOS",
            version: "1.0.0",
            kind: PluginKind::Effect,
        }
    }

    fn params(&self) -> Vec<ParamInfo> {
        vec![
            ParamInfo {
                id: "bits",
                name: "Bit depth",
                min: 1.0,
                max: 16.0,
                default: 12.0,
            },
            ParamInfo {
                id: "downsample",
                name: "Downsample factor",
                min: 1.0,
                max: 32.0,
                default: 1.0,
            },
        ]
    }

    fn set_param(&mut self, id: &str, value: f32) -> Result<(), PluginError> {
        match id {
            "bits" => {
                self.bits = value.clamp(1.0, 16.0);
                Ok(())
            }
            "downsample" => {
                self.downsample = value.clamp(1.0, 32.0);
                Ok(())
            }
            other => Err(PluginError::UnknownParam(other.to_string())),
        }
    }

    fn prepare(&mut self, _sample_rate: u32, _max_block: usize) {
        self.reset();
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let levels = 2f32.powf(self.bits - 1.0);
        for i in 0..left.len().min(right.len()) {
            self.counter += 1.0;
            if self.counter >= self.downsample {
                self.counter -= self.downsample;
                self.hold = [
                    (left[i] * levels).round() / levels,
                    (right[i] * levels).round() / levels,
                ];
            }
            left[i] = self.hold[0];
            right[i] = self.hold[1];
        }
    }

    fn reset(&mut self) {
        self.hold = [0.0; 2];
        self.counter = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicos_plugin_api::conformance;

    #[test]
    fn passes_the_conformance_harness() {
        conformance::check(Bitcrusher::factory).expect("bitcrusher must conform");
    }

    #[test]
    fn crushing_quantizes_and_holds() {
        let mut fx = Bitcrusher::new();
        fx.prepare(48_000, 512);
        fx.set_param("bits", 2.0).unwrap();
        fx.set_param("downsample", 4.0).unwrap();
        let mut l: Vec<f32> = (0..16)
            .map(|i| f32::from(i16::try_from(i).unwrap()) / 16.0)
            .collect();
        let mut r = l.clone();
        fx.process(&mut l, &mut r);
        // 2-bit quantization leaves only multiples of 0.5.
        assert!(l.iter().all(|s| (s * 2.0).fract().abs() < 1e-6));
        // Sample-and-hold: runs of 4 identical samples.
        assert!((l[0] - l[1]).abs() < f32::EPSILON);
        assert!((l[1] - l[2]).abs() < f32::EPSILON);
    }
}
