Name: flashpaste
Version: %{pkg_version}
Release: 1%{?dist}
Summary: Sub-15ms image-paste glue for terminal AI agents on GNOME Wayland
License: MIT
URL: https://github.com/NagyVikt/flashpaste
Requires: bash >= 5.0
Requires: wl-clipboard
Requires: xclip
Requires: xsel
Requires: tmux
Requires: ydotool
Requires: kitty
Requires: systemd
Requires: cairo
Requires: glib2
Requires: pango

%description
flashpaste makes screenshot-into-Claude-Code, Codex, and other terminal AI
agents work on Wayland desktops. It ships bash fallbacks, Rust fast-path
binaries, systemd user units, and the experimental overlay daemon plus
flashpaste-overlay scripting client.

%prep

%build

%install
rm -rf %{buildroot}
mkdir -p %{buildroot}%{_bindir}
mkdir -p %{buildroot}%{_datadir}/applications
mkdir -p %{buildroot}%{_datadir}/flashpaste/examples
mkdir -p %{buildroot}%{_docdir}/%{name}
mkdir -p %{buildroot}%{_licensedir}/%{name}
mkdir -p %{buildroot}%{_prefix}/lib/systemd/user

for src in %{repo_dir}/bin/*.sh %{repo_dir}/bin/wl-paste %{repo_dir}/bin/screenshot-to-clipboard; do
  [ -f "$src" ] || continue
  base="$(basename "$src" .sh)"
  install -m 0755 "$src" "%{buildroot}%{_bindir}/$base"
done

for bin in flashpasted flashpaste-dispatch flashpaste-shoot flashpaste-trigger flashpaste-mcp flashpaste-overlayd flashpaste-overlay flashpaste; do
  if [ -x "%{repo_dir}/rs/target/release/$bin" ]; then
    install -m 0755 "%{repo_dir}/rs/target/release/$bin" "%{buildroot}%{_bindir}/$bin"
  fi
done

install -m 0755 %{repo_dir}/bin/paste_image.sh %{buildroot}%{_datadir}/flashpaste/paste_image.sh

for unit in %{repo_dir}/systemd/*.service %{repo_dir}/systemd/*.path; do
  [ -f "$unit" ] || continue
  base="$(basename "$unit")"
  if [ "$base" = "flashpaste-overlayd.service" ] && { [ ! -x "%{repo_dir}/rs/target/release/flashpaste-overlayd" ] || [ ! -x "%{repo_dir}/rs/target/release/flashpaste-overlay" ]; }; then
    continue
  fi
  install -m 0644 "$unit" "%{buildroot}%{_prefix}/lib/systemd/user/$base"
done

for desk in %{repo_dir}/share/applications/*.desktop; do
  [ -f "$desk" ] || continue
  install -m 0644 "$desk" "%{buildroot}%{_datadir}/applications/$(basename "$desk")"
done

install -m 0644 %{repo_dir}/examples/tmux.conf.snippet %{buildroot}%{_datadir}/flashpaste/examples/
install -m 0644 %{repo_dir}/examples/kitty.conf.snippet %{buildroot}%{_datadir}/flashpaste/examples/
install -m 0644 %{repo_dir}/README.md %{buildroot}%{_docdir}/%{name}/README.md
install -m 0644 %{repo_dir}/ROADMAP.md %{buildroot}%{_docdir}/%{name}/ROADMAP.md 2>/dev/null || true
install -m 0644 %{repo_dir}/LICENSE %{buildroot}%{_licensedir}/%{name}/LICENSE

%post
cat <<'EOF'

flashpaste installed. To activate for your user:

  systemctl --user daemon-reload
  systemctl --user enable --now clipboard-janitor.service
  systemctl --user enable --now flashpaste-screenshot-watcher.path
EOF
if command -v flashpaste-overlayd >/dev/null 2>&1 && [ -f /usr/lib/systemd/user/flashpaste-overlayd.service ]; then
  echo "  systemctl --user enable --now flashpaste-overlayd.service"
fi
cat <<'EOF'

  cat /usr/share/flashpaste/examples/tmux.conf.snippet  >> ~/.tmux.conf
  cat /usr/share/flashpaste/examples/kitty.conf.snippet >> ~/.config/kitty/kitty.conf
  ln -sf /usr/share/flashpaste/paste_image.sh ~/paste_image.sh

Run the doctor to verify your environment:
  flashpaste-doctor

EOF

%preun
if [ "$1" = "0" ]; then
  cat <<'EOF'

flashpaste is being removed. To clean up per-user state:
  systemctl --user disable --now clipboard-janitor.service flashpaste-screenshot-watcher.path
EOF
  if [ -f /usr/lib/systemd/user/flashpaste-overlayd.service ]; then
    echo "  systemctl --user disable --now flashpaste-overlayd.service"
  fi
  cat <<'EOF'
  rm -f ~/paste_image.sh

EOF
fi

%files
%{_bindir}/*
%{_datadir}/applications/*.desktop
%{_datadir}/flashpaste
%{_prefix}/lib/systemd/user/*.service
%{_prefix}/lib/systemd/user/*.path
%doc %{_docdir}/%{name}
%license %{_licensedir}/%{name}/LICENSE
