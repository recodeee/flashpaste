# flashpaste — agent guide

Working rules for agents (Claude, Codex, etc.) editing this repository.

## Versioning + releases — non-negotiable

**Every push that bumps a version MUST be accompanied by a matching GitHub release.** Pushing a commit titled `v1.X` without a release is a defect.

Workflow for any version bump:

1. **Bump in this order:**
   - `README.md` — if a version string appears in the tagline or quick-start
   - `bin/tmux-paste-dispatch.sh` header `WORKING VERSION: v1.X — <date>` comment
   - Any other file with a `v1.X` literal — search with `git grep '^# WORKING VERSION'` and `git grep -F 'v1.'`

2. **Commit message format:**
   ```
   v1.X: <one-line summary>

   <multi-paragraph body explaining what changed and why,
   matching the style of v1.10–v1.14 in the existing history>

   Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
   ```
   Use `git log --pretty=fuller` to confirm the trailer is preserved.

3. **Tag + push:**
   ```bash
   git tag -a v1.X -m "v1.X: <one-line summary>"
   git push origin main
   git push origin v1.X
   ```

4. **Create the GitHub release immediately after pushing:**
   ```bash
   gh release create v1.X \
     --title "v1.X: <one-line summary>" \
     --notes "$(git log -1 --pretty=%B v1.X | tail -n +3)"
   ```
   The `tail -n +3` skips the title line + blank line so the body is the multi-paragraph description.
   - If this is a **breaking** change, add `--latest=true` and prepend a `### Breaking` block to the notes.
   - If experimental / not ready for general use, add `--prerelease`.

5. **Verify** with `gh release view v1.X` and `gh release list --limit 5`. The new release MUST appear in the list before considering the bump complete.

## Version-number policy

- **Patch (`v1.X.Y`)** — bash-only fixes, doc changes, .desktop tweaks, install.sh hygiene.
- **Minor (`v1.X`)** — new bash scripts, new systemd units, new env knobs.
- **Major (`v2.X`)** — reserved for the daemon (`flashpasted`) becoming the default path and the bash dispatcher becoming a fallback only. As of v1.15 the daemon is opt-in; do not cut v2.0 until the dispatcher mode-flag flips by default.

## Where work lives

```
bin/                  bash hot path (canonical, always works)
rs/                   Rust workspace — flashpaste-{common,dispatch,trigger,shoot} + flashpasted
share/applications/   NoDisplay .desktop files for surfaceless Wayland clients
systemd/              user units (clipboard-janitor, screenshot-watcher, flashpasted)
examples/             config snippets for tmux + kitty
```

When you touch the Rust code, never break the bash fallback. The trigger binary execs the bash dispatcher when the daemon is absent; the dispatch binary forks the bash dispatcher when the fast-path bails. Both fallbacks must keep working.

## Four hard-won facts the code must preserve

Stamped into `bin/tmux-paste-dispatch.sh`'s edit log; restated here so agents don't accidentally regress them:

1. **`kitty @ send-text` is the only transport** that triggers Claude Code's image-paste handler. `tmux send-keys -t pane C-v` writes the byte but the handler doesn't fire.
2. **`tmux bind -n C-v` recurses** when `\026` arrives via kitty send-text. `tmux unbind -n C-v` before send-text, rebind ~100ms later via `setsid -f sh -c 'sleep 0.1; tmux bind ...'` (detached so it survives parent exit).
3. **Wayland-authoritative `has_image` policy.** Trust Wayland if it answers; only fall back to X11 when Wayland is fully silent. mutter's X11↔Wayland bridge is sticky and X11 keeps advertising stale `image/png` after fresh text copies — trusting it produces the obs #6881 "GitHub URL → [Image #9]" bug.
4. **GNOME PrtScr saves but doesn't copy.** Auto-pickup loads `~/Pictures/Screenshots/<latest>.png` into the clipboard if ≤30s old and clipboard text is empty.

If your change might affect any of these, add a regression test (or a manual test plan) to the commit body.

## Build + test commands

```bash
# Bash syntax check
bash -n bin/*.sh install.sh bootstrap.sh

# Rust workspace
cargo build --release --manifest-path rs/Cargo.toml
cargo fmt --manifest-path rs/Cargo.toml --check
cargo clippy --release --manifest-path rs/Cargo.toml -- -D warnings

# Doctor
bash bin/flashpaste-doctor.sh
```

Do NOT run `cargo build` or `cargo update` without user approval — they hit crates.io.

## Memory-lane reminder

Per `/home/deadpool/.claude/CLAUDE.md`, three memory systems coexist on this machine. For flashpaste work, default to the file-based memory under `~/.claude-accounts/account2/projects/-home-deadpool/memory/`. Do not write project notes to claude-mem or Colony.

## Parallel-agent workflow

When dispatching multiple agents:
- Each agent owns disjoint file paths (no cross-edits).
- Pre-create shared scaffolding (e.g. `rs/Cargo.toml` workspace root) before dispatching so agents don't race on it.
- Agents that need types/wire-formats from sibling crates should duplicate small helpers inline rather than depend across in-flight crates. Refactor to a shared crate after all agents land.
- After agents return, run the sanity sweep: `bash -n` on every script, `cargo metadata --offline --no-deps` on the workspace.

## Release notes voice

Match the existing v1.10–v1.14 voice:
- Lead with what the change does.
- One paragraph per concern, no bullets unless it's a list of bug-fixes.
- Cite specific commit hashes / log timestamps / observation IDs when relevant.
- End with the side-effects the user will see ("janitor restarted; live preload script symlinked…").
- Co-Authored-By trailer for any AI-assisted change.
