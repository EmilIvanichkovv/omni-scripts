{
  lib,
  craneLib,
  git,
  sqlite,
  pkg-config,
}:

let
  src = craneLib.cleanCargoSource (
    lib.fileset.toSource {
      root = ../../..;
      fileset = lib.fileset.unions [
        ../../../Cargo.toml
        ../../../Cargo.lock
        ../../../src
        ../../../tests
      ];
    }
  );

  # Name and version come from Cargo.toml, the single source of the TUI's version.
  crateInfo = craneLib.crateNameFromCargoToml {
    cargoToml = ../../../Cargo.toml;
  };

  commonArgs = {
    inherit src;
    inherit (crateInfo) pname version;

    cargoExtraArgs = "-p git-manager";

    nativeBuildInputs = [
      git
      pkg-config
    ];

    buildInputs = [ sqlite ];

    doCheck = false;
  };

  # Build and cache dependencies separately (speeds up rebuilds)
  cargoArtifacts = craneLib.buildDepsOnly commonArgs;

in
craneLib.buildPackage (
  commonArgs
  // {
    inherit cargoArtifacts;

    meta = with lib; {
      description = "Terminal application for managing Git branches and worktrees";
      homepage = "https://github.com/EmilIvanichkovv/omni-scripts";
      license = licenses.mit;
      maintainers = [ ];
      mainProgram = "git-manager";
    };
  }
)
