{
  inputs = {
    # keep-sorted start block=yes
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
        inputs.git-hooks.flakeModule
        inputs.treefmt-nix.flakeModule
      ];

      perSystem =
        {
          config,
          pkgs,
          system,
          ...
        }:
        {
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

          packages.default = pkgs.callPackage ./. { };

          devShells.default =
            let
              overlays = [ (import inputs.rust-overlay) ];
              pkgs = import inputs.nixpkgs { inherit system overlays; };
            in
            with pkgs;
            mkShell {
              packages = [
                nixfmt
                just
                llvmPackages.libclang
                clang
                (rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
              ];
              shellHook = ''
                export LIBCLANG_PATH=${llvmPackages.libclang.lib}/lib
              '';
            };
        };
    };
}
