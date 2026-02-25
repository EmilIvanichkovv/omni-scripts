{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
  };

  outputs = inputs @ {
    nixpkgs,
    flake-parts,
    ...
  }:
    flake-parts.lib.mkFlake {inherit inputs;} {
      systems = ["x86_64-linux"];
      perSystem = {pkgs, system, self', ...}: {
        devShells = {
          default = with pkgs;
            mkShell {
              packages = [
                jq

                # Bash Automated Testing System
                bats

                # Rust toolchain
                cargo
                rustc
                rustfmt
                clippy
                rust-analyzer
                git

                # Markdown formatting tools
                nodePackages.prettier
                nodePackages.markdownlint-cli

                # Pre-commit hooks
                pre-commit
              ];

              RUST_BACKTRACE = 1;

              shellHook = ''
                # Install pre-commit hooks if not already installed
                if [ -f .pre-commit-config.yaml ] && [ ! -f .git/hooks/pre-commit ]; then
                  echo "Installing pre-commit hooks..."
                  pre-commit install
                fi
              '';
            };

          rust-tui = with pkgs;
            mkShell {
              buildInputs = [
                cargo
                rustc
                rustfmt
                clippy
                rust-analyzer
                git
              ];

              RUST_BACKTRACE = 1;
            };
        };

        packages = {
          # Bash script version (original)
          local-git-branch-cleanup = pkgs.callPackage ./pkgs/local-git-branch-cleanup {};

          # Rust TUI version (interactive)
          local-git-branch-cleanup-tui = pkgs.callPackage ./pkgs/local-git-branch-cleanup/tui.nix {
            inherit (pkgs) lib git;
            inherit (pkgs) rustPlatform;
          };

          # Default to TUI version
          default = self'.packages.local-git-branch-cleanup-tui;
        };
      };
    };
}
