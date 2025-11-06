{
  inputs = {
    # keep-sorted start block=yes
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
      inputs.nixpkgs.follows = "nixpkgs";
    };
    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
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
            git-mile = pkgs.callPackage ./. { };
            default = git-mile;
          };

          devshells.default =
            let
              overlays = [ (import inputs.rust-overlay) ];
              pkgs = import inputs.nixpkgs { inherit system overlays; };
            in
            with pkgs;
            {
              env = [
                {
                  name = "LIBCLANG_PATH";
                  value = "${llvmPackages.libclang.lib}/lib";
                }
              ];
              devshell = {
                packages = [
                  nixfmt
                  just
                  llvmPackages.libclang
                  clang
                  (rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
                ];
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
