//! Tracing and metrics setup.
//!
//! Implements the telemetry strategy of `docs/02_System_Architecture.md` §7:
//! a single global `tracing` subscriber writing human-readable output to
//! stderr, with verbosity driven by repeated `-v` flags and overridable via
//! the `RUST_LOG` environment variable.

use std::sync::Once;

use tracing_subscriber::EnvFilter;

static INIT: Once = Once::new();

/// Initializes the global tracing subscriber, writing to stderr.
///
/// `verbosity` maps to a maximum level: `0` => WARN, `1` => INFO, `2` =>
/// DEBUG, `3` or more => TRACE. When the `RUST_LOG` environment variable is
/// set it takes precedence over `verbosity`.
///
/// Idempotent: only the first call in a process installs a subscriber; later
/// calls are no-ops.
pub fn init(verbosity: u8) {
    INIT.call_once(|| {
        let default_level = match verbosity {
            0 => "warn",
            1 => "info",
            2 => "debug",
            _ => "trace",
        };
        let filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(std::io::stderr)
            .init();
    });
}

/// Initializes tracing for tests at TRACE level.
///
/// Guarded by the same [`Once`] as [`init`], so calling it from many tests
/// (or alongside [`init`]) is safe and never panics.
pub fn init_for_tests() {
    init(3);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_init_does_not_panic() {
        init(1);
        init(2);
        init_for_tests();
        init_for_tests();
    }
}
