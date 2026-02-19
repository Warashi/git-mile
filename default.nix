{
  nixpkgs ? <nixpkgs>,
  system ? builtins.currentSystem,
}:
let
  craneSrc = builtins.fetchTarball {
    url = "https://github.com/ipetkov/crane/archive/b5090e53e9d68c523a4bb9ad42b4737ee6747597.tar.gz";
    sha256 = "sha256-nGBbXvEZVe/egCPVPFcu89RFtd8Rf6J+4RFoVCFec0A=";
  };

  rustOverlaySrc = builtins.fetchTarball {
    url = "https://github.com/oxalica/rust-overlay/archive/4e8e5dfb8e649d3e05d9a173ce9a9cb0498e89c2.tar.gz";
    sha256 = "sha256-EW7xlGJnCW3mKujn/F8me52NXB4nBtabArsRNwehtHM=";
  };

  pkgs = import nixpkgs {
    inherit system;
    overlays = [ (import rustOverlaySrc) ];
  };

  toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
  craneLib = ((import craneSrc).mkLib pkgs).overrideToolchain (_: toolchain);
  src = craneLib.cleanCargoSource ./.;

  commonArgs = {
    inherit src;
    pname = "git-mile";
    version = "0.1.0";
    strictDeps = true;
    nativeBuildInputs = [ pkgs.perl ];
    cargoExtraArgs = "--package git-mile";
  };

  cargoArtifacts = craneLib.buildDepsOnly commonArgs;
in
craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts;
    meta = {
      description = "A Git extension to manage your tasks.";
      homepage = "https://github.com/Warashi/git-mile";
      license = pkgs.lib.licenses.mit;
      maintainers = [ ];
    };
  }
)
