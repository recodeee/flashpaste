# typed: false
# frozen_string_literal: true
#
# Homebrew formula for flashpaste — intended for use as a tap
# (`brew tap NagyVikt/flashpaste`), not core-brew.
#
# Linuxbrew only. macOS is unsupported because the project targets
# GNOME Wayland + ydotool + systemd --user services.
class Flashpaste < Formula
  desc "Sub-15ms image-paste glue for terminal AI agents on GNOME Wayland"
  homepage "https://github.com/NagyVikt/flashpaste"
  url "https://github.com/NagyVikt/flashpaste/archive/refs/tags/v1.32.tar.gz"
  # PLACEHOLDER — replace on each release:
  #   curl -sL https://github.com/NagyVikt/flashpaste/archive/refs/tags/v<ver>.tar.gz | sha256sum
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  license "MIT"
  head "https://github.com/NagyVikt/flashpaste.git", branch: "main"

  depends_on :linux
  depends_on "rust" => :build
  depends_on "pkg-config" => :build

  # Runtime deps available on linuxbrew. Wayland/X11 clipboard helpers:
  depends_on "cairo"
  depends_on "glib"
  depends_on "pango"
  depends_on "wl-clipboard"
  depends_on "xclip"
  depends_on "tmux"
  depends_on "kitty"

  # NOTE: `ydotool`, `ydotoold`, `xsel`, and `systemd` are NOT available as
  # Homebrew formulae on Linux. End users must install them from their
  # distro package manager (apt / pacman / dnf). The doctor command checks
  # for these at runtime and prints clear install hints.

  def install
    # Rust workspace lives under rs/.
    cd "rs" do
      system "cargo", "install", *std_cargo_args(path: "flashpaste-dispatch")
      system "cargo", "install", *std_cargo_args(path: "flashpasted")
      system "cargo", "install", *std_cargo_args(path: "flashpaste-trigger")
      system "cargo", "install", *std_cargo_args(path: "flashpaste-shoot")
      system "cargo", "install", *std_cargo_args(path: "flashpaste-mcp")
      system "cargo", "install", "--features", "wayland", *std_cargo_args(path: "flashpaste-overlayd")
      system "cargo", "install", *std_cargo_args(path: "flashpaste")
    end

    # Bash scripts → bin/ (strip .sh suffix).
    Dir["bin/*.sh"].each do |src|
      dest_name = File.basename(src, ".sh")
      bin.install src => dest_name
    end
    # Extension-less helpers already named correctly:
    %w[wl-paste screenshot-to-clipboard].each do |name|
      src = "bin/#{name}"
      bin.install src if File.exist?(src)
    end

    # paste_image.sh lives in pkgshare/ — kitty.conf snippet must point here.
    pkgshare.install "bin/paste_image.sh"

    # systemd user units — brew won't manage these, but ship them so users
    # can `ln -s "$(brew --prefix)/share/flashpaste/systemd/*.service" \
    #         ~/.config/systemd/user/`.
    (pkgshare/"systemd").install Dir["systemd/*.service"]
    (pkgshare/"systemd").install Dir["systemd/*.path"]

    # Desktop entries (most useful on a non-brew install, but ship for parity).
    (pkgshare/"applications").install Dir["share/applications/*.desktop"]

    # Examples + docs.
    (pkgshare/"examples").install "examples/tmux.conf.snippet"
    (pkgshare/"examples").install "examples/kitty.conf.snippet"
    doc.install "README.md"
    doc.install "ROADMAP.md" if File.exist?("ROADMAP.md")
  end

  def caveats
    <<~EOS
      flashpaste ships systemd --user units but Homebrew does not manage them.
      Enable them manually:

        mkdir -p ~/.config/systemd/user
        ln -sf #{opt_pkgshare}/systemd/*.service ~/.config/systemd/user/
        ln -sf #{opt_pkgshare}/systemd/*.path    ~/.config/systemd/user/
        systemctl --user daemon-reload
        systemctl --user enable --now flashpasted.service
        systemctl --user enable --now clipboard-janitor.service
        systemctl --user enable --now flashpaste-screenshot-watcher.path
        systemctl --user enable --now flashpaste-overlayd.service

      Required system packages (not on brew for Linux — use your distro):
        ydotool, ydotoold, xsel, systemd

      Append the editor snippets:
        cat #{opt_pkgshare}/examples/tmux.conf.snippet  >> ~/.tmux.conf
        cat #{opt_pkgshare}/examples/kitty.conf.snippet >> ~/.config/kitty/kitty.conf
        ln -sf #{opt_pkgshare}/paste_image.sh ~/paste_image.sh

      Verify your environment:
        flashpaste-doctor
    EOS
  end

  test do
    # Non-destructive: doctor's --help (or fallback to plain run with --no-act).
    # The bash doctor exits 0 on a help/usage path even outside a Wayland session.
    assert_match(/flashpaste/i, shell_output("#{bin}/flashpaste-doctor --help 2>&1", 0))
    # Confirm at least one Rust binary is executable and reports a version.
    assert_match(/\d+\.\d+/, shell_output("#{bin}/flashpaste --version 2>&1", 0))
  end
end
