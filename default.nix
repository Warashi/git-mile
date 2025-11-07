{
  lib,
  fetchFromGitHub,
  rustPlatform,
  perl,
}:
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "git-mile";
  version = "0.1.0";

  src = ./.;

  nativeBuildInputs = [ perl ];

  cargoLock = {
    lockFile = ./Cargo.lock;
  };

  meta = {
    description = "A Git extension to manage your tasks.";
    homepage = "https://github.com/Warashi/git-mile";
    license = lib.licenses.mit;
    maintainers = [ ];
  };
})
