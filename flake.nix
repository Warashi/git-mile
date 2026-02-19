{
  inputs = {
    # keep-sorted start block=yes
    crane = {
      url = "github:ipetkov/crane";
    };
    devshell = {
      url = "github:numtide/devshell";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };
    flake-parts = {
      url = "github:hercules-ci/flake-parts";
    };
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };
    nixpkgs = {
      url = "github:nixos/nixpkgs/nixpkgs-unstable";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs = {
        nixpkgs.follows = "nixpkgs";
      };
    };
    # keep-sorted end
  };

  outputs =
    { flake-parts, ... }@inputs:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      imports = [
        # keep-sorted start
        inputs.devshell.flakeModule
        inputs.git-hooks.flakeModule
        inputs.treefmt-nix.flakeModule
        # keep-sorted end
      ];

      perSystem =
        {
          config,
          pkgs,
          system,
          ...
        }:
        let
          toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
          craneLib = (inputs.crane.mkLib pkgs).overrideToolchain (_: toolchain);
          src = craneLib.cleanCargoSource ./.;
          commonArgs = {
            inherit src;
            pname = "git-mile";
            version = "0.1.0";
            strictDeps = true;
            nativeBuildInputs = [ pkgs.perl ];
          };
          cargoArtifacts = craneLib.buildDepsOnly (commonArgs // { cargoExtraArgs = "--package git-mile"; });
          workspaceArtifacts = craneLib.buildDepsOnly (
            commonArgs // { cargoExtraArgs = "--workspace --all-features"; }
          );
          gitMilePackage = craneLib.buildPackage (
            commonArgs
            // {
              inherit cargoArtifacts;
              cargoExtraArgs = "--package git-mile";
            }
          );
          nextestCheck = craneLib.cargoNextest (
            commonArgs
            // {
              cargoArtifacts = workspaceArtifacts;
              cargoNextestExtraArgs = "--workspace --all-features";
            }
          );
        in
        {
          _module.args.pkgs = import inputs.nixpkgs {
            inherit system;
            overlays = [ inputs.rust-overlay.overlays.default ];
            config = { };
          };

          pre-commit = {
            check.enable = true;
            settings = {
              src = ./.;
              hooks = {
                actionlint.enable = true;
                treefmt.enable = true;
              };
            };
          };

          treefmt = {
            projectRootFile = "flake.nix";
            programs = {
              nixfmt = {
                enable = true;
                strict = true;
              };
              # keep-sorted start
              keep-sorted.enable = true;
              pinact.enable = true;
              # keep-sorted end
            };
          };

          packages = rec {
            git-mile = gitMilePackage;
            default = git-mile;
          };

          checks = {
            cargo-nextest = nextestCheck;
          };

          devshells.default = with pkgs; {
            env = [
              {
                name = "LIBCLANG_PATH";
                value = "${llvmPackages.libclang.lib}/lib";
              }
            ];
            devshell = {
              packages = [
                clang
                just
                llvmPackages.libclang
                nixfmt
                tombi
                toolchain
              ]
              ++ (lib.optional stdenv.hostPlatform.isLinux cargo-llvm-cov);
              startup = {
                pre-commit = {
                  text = config.pre-commit.installationScript;
                };
              };
            };
          };
        };
    };
}
