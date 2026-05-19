# flashpaste — agent guide

Working rules for agents (Claude, Codex, etc.) editing this repository.

## Versioning + releases — non-negotiable

**Every commit titled `vX.Y` MUST be tagged and have a matching GitHub release before you end your turn.** Pushing a `v1.X` commit without a tag is a defect; pushing a tag without a release is a defect. This rule applies to any agent (Claude, Codex, etc.) and to the user themselves — when committing in parallel, agents must check at end-of-turn that there's no untagged version commit on `main`.

### TL;DR for AI agents

Run this at the end of every turn that touched this repo, **before claiming done**:

```bash
bash AGENTS-release-check.sh   # or paste-inline:
for sha in $(git log --format='%H %s' | awk '$2 ~ /^v[0-9]+\.[0-9]+/ {print $1}'); do
  tag=$(git log -1 --format=%s "$sha" | awk '{print $1}' | tr -d ':')
  if ! git tag -l "$tag" | grep -q "^$tag$"; then
    echo "MISSING TAG: $tag at $sha"
  fi
done
```

If anything prints, you have unfinished work. Tag it, push it, verify the release.

### The full workflow

The repo has `.github/workflows/release.yml` that auto-builds the .deb and publishes a GitHub release on **every `v*` tag push**. So in practice the rule reduces to: **never push a `vX.Y` commit without immediately pushing the matching tag.**

1. **Bump in this order:**
   - `README.md` — if a version string appears in the tagline or quick-start
   - `bin/tmux-paste-dispatch.sh` header `WORKING VERSION: v1.X — <date>` comment (note: historically not updated past v1.0; safe to leave alone unless the rest of the file is touched)
   - Any other file with a `v1.X` literal — `git grep -F 'v1.'`

2. **Commit message format:**

   ```text
   v1.X: <one-line summary>

   <multi-paragraph body explaining what changed and why,
   matching the style of v1.10–v1.17 in the existing history.>

   Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
   ```

   Use `git log --pretty=fuller` to confirm the trailer is preserved.

3. **Tag + push — same turn as the commit, never deferred:**

   ```bash
   git push origin main
   git tag -a v1.X -m "v1.X: <one-line summary>" <commit-sha>
   git push origin v1.X
   ```

   The `<commit-sha>` is explicit so you tag the exact commit you just pushed, not HEAD (HEAD may have moved if the user committed in parallel).

4. **The workflow handles the release.** Confirm with:

   ```bash
   gh run watch $(gh run list --workflow=release.yml --limit 1 --json databaseId -q '.[0].databaseId')
   gh release view v1.X
   ```

   Workflow runs ~3 minutes (cargo build dominates). If the workflow fails, **investigate before ending the turn** — a failed release isn't optional cleanup, it's part of the bump.

5. **If the workflow is absent** (early commits, or the file got deleted), fall back to manual:

   ```bash
   gh release create v1.X \
     --title "flashpaste v1.X" \
     --notes "$(git log -1 --pretty=%B v1.X | tail -n +3)"
   ```

   Add `--prerelease` for experimental builds; add the .deb as an asset arg when you have one built.

### Backfill policy

If you find untagged version commits in history (`v1.10`–`v1.14` are this case — they predate the .deb workflow), **do not retroactively tag them by default**. Reasons:

- The workflow doesn't exist on those commits → tag push fails the build job.
- Their build-deb.sh / Rust workspace may not exist or compile.
- Auto-generated release notes for ancient tags add noise to the Releases page.

If the user explicitly asks for backfill, push tags one-by-one and use `gh release create --notes` manually (no workflow) per tag. Verify each .deb (if any) is correct before moving on.

### Hot-spot reminders

- The user often commits in parallel with the agent. Always `git fetch origin && git log origin/main..HEAD` and `git log HEAD..origin/main` before tagging — your local HEAD may not be the version commit.
- After `git push origin main` and `git push origin v1.X`, the workflow can take 2–5 minutes. Don't claim done before `gh release view v1.X` succeeds.
- If the workflow fails for environmental reasons (transient apt fetch failure, runner restart), retry with `gh run rerun`. Don't push a v1.X+1 to "fix" a missing v1.X release.

## Version-number policy

- **Patch (`v1.X.Y`)** — bash-only fixes, doc changes, .desktop tweaks, install.sh hygiene.
- **Minor (`v1.X`)** — new bash scripts, new systemd units, new env knobs.
- **Major (`v2.X`)** — reserved for the daemon (`flashpasted`) becoming the default path and the bash dispatcher becoming a fallback only. As of v1.15 the daemon is opt-in; do not cut v2.0 until the dispatcher mode-flag flips by default.

## Where work lives

```text
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
