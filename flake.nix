{
  description = "A post-modern text editor.";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    nixCargoIntegration = {
      url = "github:yusdacra/nix-cargo-integration";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.rustOverlay.follows = "rust-overlay";
    };
  };

  outputs = inputs@{ self, nixCargoIntegration, ... }:
    nixCargoIntegration.lib.makeOutputs {
      root = ./.;
      buildPlatform = "crate2nix";
      renameOutputs = { "helix-term" = "helix"; };
      # Set default app to hx (binary is from helix-term release build)
      # Set default package to helix-term release build
      defaultOutputs = { app = "hx"; package = "helix"; };
      overrides = {
        crateOverrides = common: _: rec {
          # link languages and theme toml files since helix-loader/helix-view expects them
          helix-loader = _: { preConfigure = "ln -s ${common.root}/languages.toml .."; };
          helix-view = _: { preConfigure = "ln -s ${common.root}/{theme.toml,base16_theme.toml} .."; };
          helix-term = prev:
            let
              inherit (common) pkgs;
              grammars = pkgs.callPackage ./grammars.nix { };
              runtimeDir = pkgs.runCommand "helix-runtime" { } ''
                mkdir -p $out
                ln -s ${common.root}/runtime/* $out
                rm -r $out/grammars
                ln -s ${grammars} $out/grammars
              '';
            in
            {
              # link languages and theme toml files since helix-term expects them (for tests)
              preConfigure = "ln -s ${common.root}/{languages.toml,theme.toml,base16_theme.toml} ..";
              buildInputs = (prev.buildInputs or [ ]) ++ [ common.cCompiler.cc.lib ];
              nativeBuildInputs = [ pkgs.makeWrapper ];

              postFixup = ''
                if [ -f "$out/bin/hx" ]; then
                  wrapProgram "$out/bin/hx" --set HELIX_RUNTIME "${runtimeDir}"
                fi
              '';
            };
        };
        shell = common: prev: {
          packages = prev.packages ++ (with common.pkgs; [ lld_13 lldb cargo-tarpaulin cargo-flamegraph ]);
          env = prev.env ++ [
            { name = "HELIX_RUNTIME"; eval = "$PWD/runtime"; }
            { name = "RUST_BACKTRACE"; value = "1"; }
            { name = "RUSTFLAGS"; value = "-C link-arg=-fuse-ld=lld -C target-cpu=native -Clink-arg=-Wl,--no-rosegment"; }
          ];
        };
      };
    };
}
