{ lib, rustPlatform, git }:

rustPlatform.buildRustPackage rec {
  pname = "local-git-branch-cleanup-tui";
  version = "0.2.0";

  # Use lib.fileset to explicitly include only needed files from workspace
  src = lib.fileset.toSource {
    root = ../../rust;
    fileset = lib.fileset.unions [
      # Workspace manifest
      ../../rust/Cargo.toml
      ../../rust/Cargo.lock
      # App crate
      ../../rust/local-git-branch-cleanup-tui/Cargo.toml
      ../../rust/local-git-branch-cleanup-tui/src
      ../../rust/local-git-branch-cleanup-tui/tests
      # Library crate (dependency)
      ../../rust/omni-lib/Cargo.toml
      ../../rust/omni-lib/src
    ];
  };

  # Build only the specific package from workspace
  cargoBuildFlags = [ "-p" "local-git-branch-cleanup-tui" ];
  cargoTestFlags = [ "-p" "local-git-branch-cleanup-tui" ];

  cargoHash = "sha256-6ZA8OdA0kpu/b3OtxINq+rYc0QGxWbt7rQIzwsy1eHA=";

  nativeBuildInputs = [ git ];

  doCheck = false; # Skip tests in Nix build (they require git repos)

  meta = with lib; {
    description = "Interactive TUI for cleaning up local Git branches";
    homepage = "https://github.com/EmilIvanichkovv/omni-scripts";
    license = licenses.mit;
    maintainers = [ ];
    mainProgram = "local-git-branch-cleanup-tui";
  };
}
