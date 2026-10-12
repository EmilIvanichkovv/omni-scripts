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
      root = ../../../rust;
      fileset = lib.fileset.unions [
        ../../../rust/Cargo.toml
        ../../../rust/Cargo.lock
        ../../../rust/local-git-branch-cleanup-tui/Cargo.toml
        ../../../rust/local-git-branch-cleanup-tui/src
        ../../../rust/local-git-branch-cleanup-tui/tests
        ../../../rust/omni-lib/Cargo.toml
        ../../../rust/omni-lib/src
      ];
    }
  );

  # Name and version come from Cargo.toml, the single source of the TUI's version.
  crateInfo = craneLib.crateNameFromCargoToml {
    cargoToml = ../../../rust/local-git-branch-cleanup-tui/Cargo.toml;
  };

  commonArgs = {
    inherit src;
    inherit (crateInfo) pname version;

    cargoExtraArgs = "-p local-git-branch-cleanup-tui";

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
      description = "Interactive TUI for cleaning up local Git branches";
      homepage = "https://github.com/EmilIvanichkovv/omni-scripts";
      license = licenses.mit;
      maintainers = [ ];
      mainProgram = "local-git-branch-cleanup-tui";
    };
  }
)
