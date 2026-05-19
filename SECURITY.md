# Security Policy

## Supported versions

Only the latest minor release line is supported. Older minors do not receive backports.

| Version | Supported |
|---|---|
| `1.19.x` | yes (current) |
| `< 1.19` | no |

If you're on an older release, upgrade before reporting. The release notes are non-breaking within a minor; `apt install ./flashpaste_all.deb` or rerunning `bootstrap.sh` is sufficient.

## Reporting a vulnerability

Two channels — pick whichever is easier:

1. **GitHub Security Advisories (preferred):** [https://github.com/NagyVikt/flashpaste/security/advisories/new](https://github.com/NagyVikt/flashpaste/security/advisories/new). Private until disclosure; lets us collaborate on a fix in-repo.
2. **Email:** `webubusiness@gmail.com` with subject **`flashpaste security`**. Plain text is fine; PGP not required.

Include:

- FlashPaste version (`flashpaste version`).
- Distro + kitty + tmux versions (the same fields the bug-report template asks for).
- `flashpaste-doctor` output if relevant.
- A minimal reproducer or proof-of-concept.

We aim to acknowledge within 72 hours and ship a fix within **90 days** of the initial report. After the fix lands and the release is tagged, the advisory is published with credit (or anonymised, your call).

## Threat model

FlashPaste is a **per-user clipboard tool**. It runs entirely inside the user's login session and writes only under `~/.local/`, `$XDG_RUNTIME_DIR`, and the user's tmux/kitty IPC sockets. It does NOT need root. The systemd units are `--user` units; the bootstrap installer never touches `/etc`. The `.deb` only installs read-only files under `/usr/share/flashpaste/` and helper binaries on `$PATH` — it does not enable any system-wide service.

### In scope

- Privilege escalation. FlashPaste should never let a process gain capabilities it didn't already have under the user's session. **If you find a privesc vector, that's a real bug — please report it.**
- Sandbox / namespace escapes from a less-privileged process to the user's full session via FlashPaste's daemon or trigger sockets.
- Clipboard data exfiltration to a process outside the user's session (different uid, different login).
- Arbitrary command execution via malformed daemon-socket messages, kitty IPC payloads, or screenshot filenames.
- Path traversal / symlink attacks in `flashpaste-shoot`'s output paths or the screenshot watcher.
- Unauthenticated network sockets. There should be none; if one appears, that's a defect.

### Out of scope

- **Same-user attacks via the local IPC sockets.** The kitty IPC socket, the daemon's unix socket at `$XDG_RUNTIME_DIR/flashpaste.sock`, and the tmux server socket are all **local-user-scoped by design**. Any process running as the same uid can drive kitty, drive tmux, and read the user's clipboard. That's how Unix-domain sockets in `$XDG_RUNTIME_DIR` work — it is not a FlashPaste bug.
- The "the user runs FlashPaste as root" scenario. Don't. See `README.md` FAQ; the bootstrap installer refuses root for this reason.
- mutter / GNOME Shell / kitty / tmux / wl-clipboard / ydotool bugs. Report those upstream. FlashPaste papers over their quirks but doesn't fix them.
- "An untrusted browser tab can read my clipboard." That's the browser + compositor, not FlashPaste.

## Disclosure window

We follow a **90-day coordinated disclosure** window from initial acknowledgement. If the fix is shipped earlier, the advisory publishes earlier. If a fix needs longer than 90 days for environmental reasons (e.g. upstream blocker), we'll request an extension before the window closes and explain why.

## Diagnostic information

For most reports, the same logs the [troubleshooting flow](docs/troubleshooting.md) collects are sufficient:

```bash
flashpaste-doctor --json
tail -n 200 ~/.local/state/clipboard-pipeline.log
tail -n 200 ~/.local/state/tmux-paste.log
journalctl --user -u flashpasted -n 200 --no-pager
```

Attach only what's necessary; redact paths, hostnames, or clipboard contents that aren't relevant.
