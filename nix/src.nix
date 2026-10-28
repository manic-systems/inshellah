lib:
lib.cleanSourceWith {
  src = ../.;
  filter =
    path: type:
    lib.cleanSourceFilter path type && !(type == "directory" && baseNameOf path == "target");
}
