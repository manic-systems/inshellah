{
  lib,
  rustPlatform,
}:
rustPlatform.buildRustPackage {
  pname = "inshellah";
  version = (lib.importTOML ../Cargo.toml).package.version;
  src = lib.cleanSourceWith {
    src = ../.;
    filter =
      path: type:
      lib.cleanSourceFilter path type && !(type == "directory" && baseNameOf path == "target");
  };
  cargoLock.lockFile = ../Cargo.lock;
  meta = {
    description = "nushell completion indexer";
    mainProgram = "inshellah";
  };
}
