{
  description = "erdtree development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      forAllSystems = nixpkgs.lib.genAttrs [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          # Same toolchain as CI, plus components for local editing only.
          toolchainFile = ./rust-toolchain.toml;
          toolchain = (pkgs.rust-bin.fromRustupToolchainFile toolchainFile).override {
            extensions = (builtins.fromTOML (builtins.readFile toolchainFile)).toolchain.components ++ [
              "rust-analyzer"
              "rust-src"
            ];
          };
        in
        {
          default = pkgs.mkShell { packages = [ toolchain ]; };
        }
      );

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
