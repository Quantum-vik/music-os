# Security Policy

## Supported versions

Pre-1.0: only the latest release receives security fixes.

## Reporting a vulnerability

Please report vulnerabilities privately via GitHub Security Advisories
("Report a vulnerability" on the repository's Security tab). Do not open
public issues for security reports. You will get an acknowledgement within
7 days and a fix or mitigation plan within 30.

## Scope notes

- Loading a CLAP plugin executes code from that library by design; only load
  plugins you trust. `music plugins --probe` runs the library's entry point.
- Project bundles (`.musicos`) are parsed defensively (fuzz-tested to return
  errors, never panic), but render/playback of hostile bundles is not a
  sandbox boundary.
- Dependency advisories are tracked in CI via cargo-audit and cargo-deny;
  accepted exceptions live in `.cargo/audit.toml` and `deny.toml` with
  justifications.
