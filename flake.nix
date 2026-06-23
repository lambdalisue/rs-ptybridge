{
  description = "Agent-agnostic, editor-agnostic headless terminal CLI that streams screen state as a JSONL protocol";

  # Pull prebuilt outputs from the project's binary cache so `nix develop` and
  # `nix run` skip recompiling dependencies. CI populates it (see the nix job in
  # .github/workflows/ci.yml).
  nixConfig = {
    extra-substituters = [ "https://ptybridge.cachix.org" ];
    extra-trusted-public-keys = [ "ptybridge.cachix.org-1:De4ZCB3lToRa/TSC/6PGuTAyPbF6GG8KWUUnN8mT8qg=" ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
    }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];
      eachSystem = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = eachSystem (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          inherit (pkgs) lib;
          craneLib = crane.mkLib pkgs;
          isDarwin = pkgs.stdenv.hostPlatform.isDarwin;

          cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);

          commonArgs = {
            pname = cargoToml.package.name;
            version = cargoToml.package.version;
            src = craneLib.cleanCargoSource ./.;
            strictDeps = true;
            # PTY-spawning integration tests need /dev/ptmx, which the Nix
            # sandbox does not provide. CI runs the full test matrix across
            # Linux, macOS, and Windows instead.
            doCheck = false;
            buildInputs = lib.optionals isDarwin [ pkgs.libiconv ];
          };

          cargoArtifacts = craneLib.buildDepsOnly commonArgs;

          ptybridge = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              meta = {
                inherit (cargoToml.package) description;
                homepage = cargoToml.package.repository;
                license = with lib.licenses; [
                  mit
                  asl20
                ];
                mainProgram = "ptybridge";
              };
            }
          );
        in
        {
          default = ptybridge;
          inherit ptybridge;
        }
      );

      apps = eachSystem (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.ptybridge}/bin/ptybridge";
        };
      });

      devShells = eachSystem (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          inherit (pkgs) lib;
          craneLib = crane.mkLib pkgs;
          isDarwin = pkgs.stdenv.hostPlatform.isDarwin;
        in
        {
          default = craneLib.devShell {
            # craneLib.devShell already provides cargo, rustc, rustfmt, clippy,
            # and cargo-nextest. Add only the tools beyond the Rust toolchain.
            packages = [
              pkgs.rust-analyzer # IDE support
              pkgs.deno # reference hosts under examples/
              pkgs.just # task runner
            ]
            ++ lib.optionals isDarwin [ pkgs.libiconv ];

            # Expose the Rust standard library sources for rust-analyzer.
            RUST_SRC_PATH = "${pkgs.rustPlatform.rustLibSrc}";

            shellHook = ''
              echo "🦀 ptybridge development environment"
              echo "  - cargo:   $(cargo --version)"
              echo "  - rustc:   $(rustc --version)"
              echo "  - deno:    $(deno --version | head -n1)"
              echo "  - just:    $(just --version)"
            '';
          };
        }
      );
    };
}
