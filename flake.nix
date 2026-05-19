{
  description = "Sub-15ms image-paste glue for terminal AI agents on GNOME Wayland";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        lib = pkgs.lib;

        # Runtime helpers expected on $PATH at flashpaste runtime.
        # `ydotool` is the critical one — it powers the synthetic Ctrl+V.
        runtimeDeps = with pkgs; [
          wl-clipboard
          xclip
          xsel
          ydotool
          tmux
          kitty
          bash
          coreutils
          util-linux
        ];

        flashpaste = pkgs.rustPlatform.buildRustPackage {
          pname = "flashpaste";
          version = "1.15";
          src = ./.;

          # Cargo workspace lives under rs/.
          sourceRoot = "source/rs";

          # MAINTAINER: replace fakeHash with the real hash after the first
          # `nix build` — Nix will print the expected value in the error
          # message. Re-run `nix build` until it succeeds.
          cargoLock = {
            lockFile = ./rs/Cargo.lock;
          };
          cargoHash = lib.fakeHash;

          nativeBuildInputs = with pkgs; [
            pkg-config
            makeWrapper
          ];

          buildInputs = with pkgs; [
            # Add any native libs the Rust crates link against here.
            # (Currently the workspace is pure-Rust; left for future expansion.)
          ];

          # Build all binaries the workspace produces.
          cargoBuildFlags = [ "--workspace" ];

          # Tests need a live Wayland/ydotool session — skip in the sandbox.
          doCheck = false;

          # Ship the bash scripts + systemd units + examples alongside the
          # Rust binaries. `buildRustPackage` already installed the Rust
          # bins into $out/bin/ before `postInstall` runs.
          postInstall = ''
            # Bash helpers from bin/  →  $out/bin/  (strip .sh)
            for src in $src/../bin/*.sh; do
              [ -f "$src" ] || continue
              base="$(basename "$src" .sh)"
              install -Dm0755 "$src" "$out/bin/$base"
            done
            for src in $src/../bin/wl-paste $src/../bin/screenshot-to-clipboard; do
              [ -f "$src" ] || continue
              install -Dm0755 "$src" "$out/bin/$(basename "$src")"
            done

            # paste_image.sh shared payload.
            install -Dm0755 $src/../bin/paste_image.sh \
              "$out/share/flashpaste/paste_image.sh"

            # systemd user units (Nix users typically wire these via
            # home-manager; ship them under share/ for reference).
            for unit in $src/../systemd/clipboard-janitor.service \
                        $src/../systemd/flashpaste-screenshot-watcher.path \
                        $src/../systemd/flashpaste-screenshot-watcher.service \
                        $src/../systemd/flashpasted.service; do
              [ -f "$unit" ] || continue
              install -Dm0644 "$unit" "$out/lib/systemd/user/$(basename "$unit")"
            done

            # Desktop entries.
            for desk in $src/../share/applications/*.desktop; do
              [ -f "$desk" ] || continue
              install -Dm0644 "$desk" "$out/share/applications/$(basename "$desk")"
            done

            # Examples + docs.
            install -Dm0644 $src/../examples/tmux.conf.snippet \
              "$out/share/flashpaste/examples/tmux.conf.snippet"
            install -Dm0644 $src/../examples/kitty.conf.snippet \
              "$out/share/flashpaste/examples/kitty.conf.snippet"
            install -Dm0644 $src/../README.md \
              "$out/share/doc/flashpaste/README.md"

            # Wrap the user-facing entrypoints so runtime deps resolve
            # without polluting the global environment.
            for bin in flashpaste flashpasted flashpaste-dispatch \
                       flashpaste-trigger flashpaste-shoot flashpaste-mcp \
                       flashpaste-doctor; do
              if [ -x "$out/bin/$bin" ]; then
                wrapProgram "$out/bin/$bin" \
                  --prefix PATH : ${lib.makeBinPath runtimeDeps}
              fi
            done
          '';

          meta = with lib; {
            description = "Sub-15ms image-paste glue for terminal AI agents on GNOME Wayland";
            homepage = "https://github.com/NagyVikt/flashpaste";
            license = licenses.mit;
            maintainers = [ ];
            platforms = platforms.linux;
            mainProgram = "flashpaste";
          };
        };
      in
      {
        packages = {
          default = flashpaste;
          flashpaste = flashpaste;
        };

        apps = {
          default = {
            type = "app";
            program = "${flashpaste}/bin/flashpaste-doctor";
          };
          doctor = {
            type = "app";
            program = "${flashpaste}/bin/flashpaste-doctor";
          };
          daemon = {
            type = "app";
            program = "${flashpaste}/bin/flashpasted";
          };
        };

        devShells.default = pkgs.mkShell {
          name = "flashpaste-dev";
          inputsFrom = [ flashpaste ];
          packages = with pkgs; [
            # Rust toolchain for hacking on the workspace.
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            # Runtime deps so `cargo run` works inside the shell.
          ] ++ runtimeDeps;

          shellHook = ''
            echo "flashpaste devshell — cargo + runtime deps on \$PATH"
            echo "  cargo build --release --manifest-path rs/Cargo.toml"
          '';
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
