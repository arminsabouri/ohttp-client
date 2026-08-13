{
  description = "Development shell for ohttp-client";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { nixpkgs, rust-overlay, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
        # Single source of truth: `rust-version` in Cargo.toml.
        msrv = cargoToml.package.rust-version;

        # Stable toolchain used for day-to-day work, plus the wasm target the
        # `check-wasm` / `build-wasm` recipes need.
        rustStable = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rustfmt" "clippy" ];
          targets = [ "wasm32-unknown-unknown" ];
        };

        # Second toolchain, kept off PATH, so `just msrv` can check the crate
        # against its declared floor without rustup.
        rustMsrv = pkgs.rust-bin.stable.${msrv}.default.override {
          targets = [ "wasm32-unknown-unknown" ];
        };

        # Everything `just check` needs except the MSRV toolchain.
        stableTools = [
          rustStable
          pkgs.cargo-audit
          pkgs.wasm-pack
          pkgs.binaryen # wasm-opt, so wasm-pack does not fetch its own
          pkgs.nodejs_22
          pkgs.just
          pkgs.pkg-config
        ];

        # `just msrv` reads this instead of shelling out to rustup.
        msrvEnv = {
          MSRV_CARGO = "${rustMsrv}/bin/cargo";
        };
      in
      {
        devShells = {
          # Local development: both toolchains, so `just check` runs end to end.
          default = pkgs.mkShell (msrvEnv // {
            packages = stableTools;

            RUST_SRC_PATH = "${rustStable}/lib/rustlib/src/rust/library";

            shellHook = ''
              echo "ohttp-client dev shell — rust $(rustc --version | cut -d' ' -f2), msrv ${msrv}"
              echo "run 'just' to list recipes"
            '';
          });

          ci = pkgs.mkShell {
            packages = stableTools;
          };

          msrv = pkgs.mkShell (msrvEnv // {
            packages = [ pkgs.just pkgs.pkg-config ];
          });
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
