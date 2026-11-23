{
  lib,
  stdenv,
  buildPackages,
  rustPlatform,
  installShellFiles,
  nushell,
}:
rustPlatform.buildRustPackage {
  pname = "inshellah";
  version = (lib.importTOML ../Cargo.toml).package.version;
  src = import ./src.nix lib;
  cargoLock.lockFile = ../Cargo.lock;
  nativeBuildInputs = [ installShellFiles ];
  nativeCheckInputs = [ nushell ];

  postInstall = lib.optionalString (stdenv.hostPlatform.emulatorAvailable buildPackages) (
    let
      inshellah = "${stdenv.hostPlatform.emulator buildPackages} $out/bin/inshellah";
    in
    ''
      installShellCompletion --cmd inshellah \
        --bash    <(COMPLETE=bash ${inshellah}) \
        --zsh     <(COMPLETE=zsh  ${inshellah}) \
        --fish    <(COMPLETE=fish ${inshellah}) \
        --nushell <(COMPLETE=nu   ${inshellah})
    ''
  );

  meta = {
    description = "nushell completion indexer";
    mainProgram = "inshellah";
  };
}
