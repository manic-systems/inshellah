{
  lib,
  rustPlatform,
  nushell,
}:
rustPlatform.buildRustPackage {
  pname = "inshellah";
  version = (lib.importTOML ../Cargo.toml).package.version;
  src = import ./src.nix lib;
  cargoLock.lockFile = ../Cargo.lock;
  nativeCheckInputs = [ nushell ];
  meta = {
    description = "nushell completion indexer";
    mainProgram = "inshellah";
  };
}
