{
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "inshellah";
  version = (lib.importTOML ../Cargo.toml).package.version;
  src = lib.cleanSource ../.;
  cargoLock.lockFile = ../Cargo.lock;
  meta = {
    description = "nushell completion indexer";
    mainProgram = "inshellah";
  };
}
