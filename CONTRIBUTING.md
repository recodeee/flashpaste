# Contributing to FlashPaste

Human contributor guide. AI agents: read [`AGENTS.md`](AGENTS.md) instead — it is the authoritative source for agent workflow, release policy, and the version-bump rules. Do not duplicate that content here.

## Scope

FlashPaste is a clipboard glue tool for **GNOME Wayland + kitty + tmux**. Patches that broaden the supported stack (wlroots, KDE, foot, alacritty) are welcome but must not break the canonical stack.

## Dev environment

```bash
git clone https://github.com/NagyVikt/flashpaste.git
cd flashpaste
./bin/flashpaste-doctor.sh    # 13-probe pre-flight; touches nothing
./install.sh                  # symlinks into ~/.local/bin/, drops user systemd units
```

To build the Rust tiers from source:

```bash
cd rs
cargo build --release
install -m 0755 target/release/flashpaste-{dispatch,trigger,shoot} \
                target/release/flashpasted \
                ~/.local/bin/
```

`cargo build` hits crates.io — run it interactively, not in unattended scripts.

## The four hard-won facts the code must preserve

Quoted verbatim from [`AGENTS.md`](AGENTS.md). If a patch might affect any of them, attach a regression test or a manual test plan to the commit body.

1. **`kitty @ send-text` is the only transport** that triggers Claude Code's image-paste handler. `tmux send-keys -t pane C-v` writes the byte but the handler doesn't fire.
2. **`tmux bind -n C-v` recurses** when `\026` arrives via kitty send-text. `tmux unbind -n C-v` before send-text, rebind ~100ms later via `setsid -f sh -c 'sleep 0.1; tmux bind ...'` (detached so it survives parent exit).
3. **Wayland-authoritative `has_image` policy.** Trust Wayland if it answers; only fall back to X11 when Wayland is fully silent. mutter's X11↔Wayland bridge is sticky and X11 keeps advertising stale `image/png` after fresh text copies — trusting it produces the obs #6881 "GitHub URL → [Image #9]" bug.
4. **GNOME PrtScr saves but doesn't copy.** Auto-pickup loads `~/Pictures/Screenshots/<latest>.png` into the clipboard if ≤30s old and clipboard text is empty.

## Commit message format

Follow the existing v1.10–v1.19 voice. Subject + body + trailer:

```text
v1.X: <one-line summary>

<multi-paragraph body explaining what changed and why,
matching the style of v1.10–v1.17 in the existing history.>

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

The `Co-Authored-By` trailer is required for any AI-assisted change. Confirm it survived with `git log --pretty=fuller`.

## Versioning

- **Patch (`v1.X.Y`)** — bash-only fixes, doc changes, .desktop tweaks, install.sh hygiene.
- **Minor (`v1.X`)** — new bash scripts, new systemd units, new env knobs.
- **Major (`v2.X`)** — reserved for the daemon becoming the default path and the bash dispatcher becoming a fallback only. Do not cut v2.0 until the dispatcher mode-flag flips by default.

## Release workflow

Every commit titled `vX.Y` MUST be tagged and have a matching GitHub release before the turn ends. The `.github/workflows/release.yml` workflow auto-builds the `.deb` and publishes the release on every `v*` tag push.

```bash
git push origin main
git tag -a v1.X -m "v1.X: <one-line summary>" <commit-sha>
git push origin v1.X
gh run watch $(gh run list --workflow=release.yml --limit 1 --json databaseId -q '.[0].databaseId')
gh release view v1.X
```

Verify with the tag-audit:

```bash
bash AGENTS-release-check.sh
```

If anything prints, you have unfinished work. Full policy in [`AGENTS.md`](AGENTS.md).

## Tests + lint

Before opening a PR:

```bash
# Bash syntax check
bash -n bin/*.sh install.sh bootstrap.sh

# Rust workspace (gated on user approval — hits crates.io)
cargo build   --release --manifest-path rs/Cargo.toml
cargo fmt              --manifest-path rs/Cargo.toml --check
cargo clippy --release --manifest-path rs/Cargo.toml -- -D warnings

# Pre-flight
flashpaste-doctor
```

Run `flashpaste-doctor` on your own machine before submitting — every PR template asks for its output.

## Reporting bugs

Use the GitHub issue forms:

- [`bug_report.yml`](.github/ISSUE_TEMPLATE/bug_report.yml) — image paste failing, dock flicker, daemon crash, etc.
- [`feature_request.yml`](.github/ISSUE_TEMPLATE/feature_request.yml) — new capability or transport.

Security vulnerabilities go through [`SECURITY.md`](SECURITY.md), not the public tracker.

## Code of Conduct

By participating you agree to the [Contributor Covenant 2.1](CODE_OF_CONDUCT.md).

## License

By contributing you agree your work is licensed under MIT — see [`LICENSE`](LICENSE).
