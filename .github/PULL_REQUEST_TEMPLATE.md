<!--
Thanks for the PR. Please complete the checklist before requesting review.
See AGENTS.md for the full contributor + release workflow.
-->

## Why

<!-- One paragraph: what user-visible problem does this PR address? -->

## What changed

<!-- Bullet points or a paragraph. Include file paths where useful. -->

## How tested

<!-- Commands you ran, scenarios you walked through, latency numbers if relevant. -->

## Checklist

- [ ] Bash syntax + shellcheck pass through the `Lint` workflow
- [ ] `cargo build --release --locked --manifest-path rs/Cargo.toml` passes (if any Rust changed)
- [ ] `cargo fmt --manifest-path rs/Cargo.toml --all --check` passes (if any Rust changed)
- [ ] `cargo clippy --release --locked --manifest-path rs/Cargo.toml -- -D warnings` passes (if any Rust changed)
- [ ] `flashpaste-doctor` runs clean against the changes
- [ ] If this is a `vX.Y` bump: tag pushed in the same turn (see [AGENTS.md](../AGENTS.md))
- [ ] `CHANGELOG.md` updated under `## [Unreleased]`
- [ ] Relevant docs in `docs/` updated
- [ ] None of the four hard-won facts (see AGENTS.md) regressed

## Type of change

<!-- Mark with [x] -->

- [ ] Bug fix (no API change)
- [ ] Feature (additive, no breakage)
- [ ] Breaking change (requires a major version bump)
- [ ] Docs only
- [ ] Build / packaging / CI only
