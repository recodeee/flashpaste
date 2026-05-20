# flashpaste — AUR packaging

Two PKGBUILDs live here:

| File           | AUR name         | Source                                                  |
| -------------- | ---------------- | ------------------------------------------------------- |
| `PKGBUILD`     | `flashpaste`     | GitHub release tarball (`v$pkgver.tar.gz`)              |
| `PKGBUILD-git` | `flashpaste-git` | `git+https://github.com/NagyVikt/flashpaste.git` (HEAD) |

Both install:

- Rust binaries (`flashpaste`, `flashpasted`, `flashpaste-dispatch`, `flashpaste-trigger`, `flashpaste-shoot`, `flashpaste-mcp`, `flashpaste-overlayd`, `flashpaste-overlay`) → `/usr/bin/`
- Bash scripts from `bin/` → `/usr/bin/` (stripped of the `.sh` suffix to keep `$PATH` clean)
- systemd **user** units → `/usr/lib/systemd/user/`
- `.desktop` entries → `/usr/share/applications/`
- Examples + docs → `/usr/share/flashpaste/`

## End-user install

```sh
# Stable (recommended):
paru -S flashpaste
# or
yay -S flashpaste

# Bleeding-edge (tracks main):
paru -S flashpaste-git
```

After install, activate the per-user services (the package cannot do this for you — systemd `--user` units can't be enabled from a root `pacman` transaction):

```sh
systemctl --user daemon-reload
systemctl --user enable --now flashpasted.service
systemctl --user enable --now flashpaste-overlayd.service
systemctl --user enable --now clipboard-janitor.service
systemctl --user enable --now flashpaste-screenshot-watcher.path

# Wire the tmux + kitty snippets into your dotfiles:
cat /usr/share/flashpaste/examples/tmux.conf.snippet  >> ~/.tmux.conf
cat /usr/share/flashpaste/examples/kitty.conf.snippet >> ~/.config/kitty/kitty.conf
ln -sf /usr/share/flashpaste/paste_image.sh ~/paste_image.sh

flashpaste-doctor
```

## Local install (no AUR account needed)

```sh
git clone https://github.com/NagyVikt/flashpaste.git
cd flashpaste/packaging/aur
cp PKGBUILD /tmp/flashpaste-build/PKGBUILD
cd /tmp/flashpaste-build
makepkg -si
```

`-s` pulls missing makedeps via `pacman`; `-i` installs the resulting `.pkg.tar.zst`.

## Maintainer: publishing to the AUR

1. Bump `pkgver` and `pkgrel` in `PKGBUILD` for each release.
2. Regenerate `.SRCINFO`:

   ```sh
   cd packaging/aur
   makepkg --printsrcinfo > .SRCINFO
   ```

3. Push to the AUR repo (one-time setup: `git clone ssh://aur@aur.archlinux.org/flashpaste.git`):

   ```sh
   cp PKGBUILD .SRCINFO ~/aur/flashpaste/
   cd ~/aur/flashpaste
   git add PKGBUILD .SRCINFO
   git commit -m "flashpaste $pkgver-$pkgrel"
   git push
   ```

   Or, if using [`aurpublish`](https://wiki.archlinux.org/title/AUR_submission_guidelines) (git subtree helper for AUR maintainers — see Arch Wiki for current implementations):

   ```sh
   aurpublish flashpaste
   ```

4. Repeat for `flashpaste-git` against the `flashpaste-git` AUR repo.

## Notes

- `b2sums=('SKIP')` is intentional: the GitHub-generated tarball hash changes when GitHub re-generates the archive. If you want a hard lock, switch to `b2sums=("$(b2sum < flashpaste-$pkgver.tar.gz)")` after downloading the release.
- The PKGBUILDs do NOT run `cargo build` against the network during `package()` — `prepare()` calls `cargo fetch --locked` and the build is `--frozen` afterwards, so AUR chroot builds work.
- `check()` runs `cargo test --release` but tolerates failure (`|| true`) because some integration tests need a live Wayland/`ydotool` session that's not present in `makepkg` chroots.
