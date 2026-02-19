{
  system ? builtins.currentSystem,
  flakeLock ? builtins.fromJSON (builtins.readFile ./flake.lock),
}:
let
  lockNode =
    name:
    let
      node = flakeLock.nodes.${name}.locked;
    in
    if node.type != "github" then
      throw "default.nix expects ${name} in flake.lock to be a github source"
    else
      node;

  sourceFromLock =
    name:
    let
      node = lockNode name;
    in
    builtins.fetchTarball {
      url = "https://github.com/${node.owner}/${node.repo}/archive/${node.rev}.tar.gz";
      sha256 = node.narHash;
    };

  nixpkgsSrc = sourceFromLock "nixpkgs";
  craneSrc = sourceFromLock "crane";
  rustOverlaySrc = sourceFromLock "rust-overlay";

  pkgs = import nixpkgsSrc {
    inherit system;
    overlays = [ (import rustOverlaySrc) ];
  };

  toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
  craneLib = (import craneSrc { inherit pkgs; }).overrideToolchain (_: toolchain);
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
