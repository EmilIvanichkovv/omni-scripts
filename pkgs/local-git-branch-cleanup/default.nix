{ lib, rustPlatform, git }:

rustPlatform.buildRustPackage rec {
  pname = "local-git-branch-cleanup-tui";
  version = "0.2.0";

  # Use lib.fileset to explicitly include only needed files
  src = lib.fileset.toSource {
    root = ../../rust;
    fileset = lib.fileset.unions [
      ../../rust/Cargo.toml
      ../../rust/Cargo.lock
      ../../rust/src
      ../../rust/tests
    ];
  };

  cargoHash = "sha256-lfwV+92/EhxvprDIGAmk4SgU5/ZCbueOaBgEGJ5f9e8=";

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
