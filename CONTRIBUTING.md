# Contributing to MusicOS

## Ground rules

1. **Docs before code.** Features follow the 11-step workflow in
   [docs/12](docs/12_Development_Roadmap.md) (research → design → review → traits →
   ports → adapters → tests → implement → benchmark → document → refactor). Significant
   or hard-to-reverse decisions get an [ADR](docs/adr/template.md) first.
2. **The dependency rule is law.** Dependencies point inward
   ([docs/02 §2](docs/02_System_Architecture.md)); `scripts/check_layers.py` enforces it.
   New crates must be classified in that script's layer map — that classification is
   part of code review.
3. **The audio thread is sacred.** No allocation, locks, I/O, or logging in RT code
   ([docs/10 §4](docs/10_Thread_Model.md)).
4. **AI acts through commands only** ([docs/06 §1](docs/06_AI_Runtime.md)).

## Quality gates (all must be green — no warnings)

```sh
just ci   # = fmt check, layer check, clippy -D warnings, tests, docs, audit, deny
```

- Every public API is documented (`missing_docs` is deny at the workspace level).
- Domain logic changes come with tests (unit + property tests where meaningful).
- Performance-sensitive changes come with a criterion benchmark; >5% regression on a
  tracked metric needs written justification ([docs/11 §4](docs/11_Performance_Goals.md)).

## Setup

```sh
git clone <repo> && cd music-os
just setup
```

Windows: install rustup, just, python3, and the cargo tools listed in
`scripts/setup.sh` manually (or use WSL).

## Commits & PRs

- Small, reviewable PRs; one concern per PR.
- PR description states which design doc section it implements.
- CI must be green; hooks run fmt + layer checks locally (`git config core.hooksPath`
  is set by `just setup`).

## License

By contributing, you agree your contributions are dual-licensed MIT OR Apache-2.0.
