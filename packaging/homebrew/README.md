# flashpaste — Homebrew packaging

This formula is intended for use as a **tap**, not for submission to
`homebrew/homebrew-core`. Reasons:

- flashpaste targets Linux only (GNOME Wayland + `ydotool` + systemd `--user`).
- Several runtime deps (`ydotool`, `ydotoold`, `xsel`, `systemd`) have no
  Linuxbrew formulae, so the install always requires distro packages.
- Homebrew can't manage systemd `--user` services.

## Maintainer: creating the tap

One-time setup. The tap is a separate repo, conventionally named
`homebrew-<tapname>`.

```sh
# 1. Create the tap repo on GitHub: NagyVikt/homebrew-flashpaste
gh repo create NagyVikt/homebrew-flashpaste --public \
  --description "Homebrew tap for flashpaste"

# 2. Clone it locally and add the formula.
git clone git@github.com:NagyVikt/homebrew-flashpaste.git
cd homebrew-flashpaste
mkdir -p Formula
cp /path/to/flashpaste/packaging/homebrew/flashpaste.rb Formula/

# 3. Update sha256 in Formula/flashpaste.rb for the release:
curl -sL https://github.com/NagyVikt/flashpaste/archive/refs/tags/v1.15.tar.gz \
  | sha256sum

# 4. Commit + push.
git add Formula/flashpaste.rb
git commit -m "flashpaste 1.15"
git push
```

On each new flashpaste release: bump `url`, bump `sha256`, push.

## End-user install

```sh
brew tap NagyVikt/flashpaste
brew install flashpaste

# Or in one step:
brew install NagyVikt/flashpaste/flashpaste
```

### Required distro packages (NOT installed by brew)

These have no Homebrew formula on Linux. Install via your distro:

```sh
# Debian / Ubuntu:
sudo apt install ydotool xsel

# Arch:
sudo pacman -S ydotool xsel

# Fedora:
sudo dnf install ydotool xsel
```

`systemd` itself is provided by the base OS.

### Post-install activation

Homebrew does not manage systemd `--user` services. Wire them up manually:

```sh
mkdir -p ~/.config/systemd/user
ln -sf "$(brew --prefix)/share/flashpaste/systemd/"*.service ~/.config/systemd/user/
ln -sf "$(brew --prefix)/share/flashpaste/systemd/"*.path    ~/.config/systemd/user/

systemctl --user daemon-reload
systemctl --user enable --now flashpasted.service
systemctl --user enable --now clipboard-janitor.service
systemctl --user enable --now flashpaste-screenshot-watcher.path

# Editor + paste-image wiring:
cat "$(brew --prefix)/share/flashpaste/examples/tmux.conf.snippet"  >> ~/.tmux.conf
cat "$(brew --prefix)/share/flashpaste/examples/kitty.conf.snippet" >> ~/.config/kitty/kitty.conf
ln -sf "$(brew --prefix)/share/flashpaste/paste_image.sh" ~/paste_image.sh

flashpaste-doctor
```

## Notes

- The `sha256` in the formula is a 64-zero placeholder. The first commit to
  the tap MUST replace it with a real digest (see step 3 above).
- The `test do` block runs `flashpaste-doctor --help` and `flashpaste --version`
  — both should succeed without a Wayland session, so the formula is testable
  in a brew CI environment.
- `head` is wired to `main` so users can `brew install --HEAD flashpaste`.
