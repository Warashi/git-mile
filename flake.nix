{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    {
      self,
      nixpkgs,
      rust-overlay,
      flake-utils,
      ...
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
      in
      {
        devShells.default =
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
      }
    );
}
