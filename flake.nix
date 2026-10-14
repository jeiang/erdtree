{
  description = "erdtree: a multithreaded filesystem and disk-usage tool";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      rust-overlay,
    }:
    let
      inherit (nixpkgs) lib;
      forAllSystems = lib.genAttrs [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];

      systems = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          toolchainFile = ./rust-toolchain.toml;
          # Same toolchain as CI: rust-toolchain.toml.
          craneLib = (crane.mkLib pkgs).overrideToolchain (
            p: p.rust-bin.fromRustupToolchainFile toolchainFile
          );

          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [
              (craneLib.fileset.commonCargoSources ./.)
              # Integration test fixtures are not Cargo sources.
              ./tests
            ];
          };

          commonArgs = {
            inherit src;
            strictDeps = true;
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          erdtree = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              # Tests run in the separate `test` check.
              doCheck = false;
              nativeBuildInputs = [ pkgs.installShellFiles ];
              # Completions come from the built binary, so skip them when it cannot run here.
              postInstall = lib.optionalString (pkgs.stdenv.buildPlatform.canExecute pkgs.stdenv.hostPlatform) ''
                installShellCompletion --cmd erd \
                  --bash <($out/bin/erd --no-config --completions bash) \
                  --fish <($out/bin/erd --no-config --completions fish) \
                  --zsh <($out/bin/erd --no-config --completions zsh)
              '';
              meta.mainProgram = "erd";
            }
          );
        in
        {
          inherit craneLib erdtree;

          checks = {
            inherit erdtree;
            clippy = craneLib.cargoClippy (
              commonArgs
              // {
                inherit cargoArtifacts;
                cargoClippyExtraArgs = "--all-targets -- -D warnings";
              }
            );
            fmt = craneLib.cargoFmt { inherit src; };
            test = craneLib.cargoTest (commonArgs // { inherit cargoArtifacts; });
          };

          # Same toolchain as CI, plus components for local editing only.
          devToolchain = (pkgs.rust-bin.fromRustupToolchainFile toolchainFile).override {
            extensions = (lib.importTOML toolchainFile).toolchain.components ++ [
              "rust-analyzer"
              "rust-src"
            ];
          };
        }
      );
    in
    {
      packages = lib.mapAttrs (_: s: {
        inherit (s) erdtree;
        default = s.erdtree;
      }) systems;

      checks = lib.mapAttrs (_: s: s.checks) systems;

      devShells = lib.mapAttrs (_: s: {
        default = (s.craneLib.overrideToolchain s.devToolchain).devShell {
          # Build inputs of every check, so `cargo` sees the same environment.
          inherit (s) checks;
        };
      }) systems;

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
