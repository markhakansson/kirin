{
  description = "kirin";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs { inherit system overlays; };
        rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

        kirin = pkgs.rustPlatform.buildRustPackage {
          pname = "kirin";
          version = "0.1.0";
          src = self;
          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "kiutils_kicad-0.3.0" = "sha256-crJmgSjBGaonU6awQ2ilq3u7cLYftdwWm/4uMbL+3y8=";
              "kiutils_sexpr-0.1.1" = "sha256-crJmgSjBGaonU6awQ2ilq3u7cLYftdwWm/4uMbL+3y8=";
            };
          };
        };
      in
      {
        packages = {
          inherit kirin;
          default = kirin;
        };

        devShells.default = pkgs.mkShell {
          buildInputs = [
            rustToolchain
            pkgs.rust-analyzer
            pkgs.pkg-config
            pkgs.kicad
          ];
        };
      }
    );
}
