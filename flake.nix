{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = inputs @ {
    nixpkgs,
    flake-parts,
    git-hooks,
    ...
  }:
    flake-parts.lib.mkFlake {inherit inputs;} {
      systems = ["x86_64-linux"];
      perSystem = {
        pkgs,
        system,
        self',
        ...
      }: let
        # Define pre-commit hooks using git-hooks.nix
        pre-commit-check = git-hooks.lib.${system}.run {
          src = ./.;
          hooks = {
            # General hooks
            trailing-whitespace = {
              enable = true;
              name = "trim trailing whitespace";
              entry = "${pkgs.python3Packages.pre-commit-hooks}/bin/trailing-whitespace-fixer";
              types = ["text"];
            };
            end-of-file-fixer = {
              enable = true;
              name = "fix end of files";
              entry = "${pkgs.python3Packages.pre-commit-hooks}/bin/end-of-file-fixer";
              types = ["text"];
            };
            check-yaml = {
              enable = true;
              name = "check yaml";
              entry = "${pkgs.python3Packages.pre-commit-hooks}/bin/check-yaml";
              types = ["yaml"];
            };
            check-toml = {
              enable = true;
              name = "check toml";
              entry = "${pkgs.python3Packages.pre-commit-hooks}/bin/check-toml";
              types = ["toml"];
            };
            check-merge-conflict = {
              enable = true;
              name = "check for merge conflicts";
              entry = "${pkgs.python3Packages.pre-commit-hooks}/bin/check-merge-conflict";
              types = ["text"];
            };

            # Rust formatting
            rustfmt = {
              enable = true;
              name = "cargo fmt";
              entry = "bash -c 'cd rust && cargo fmt --all'";
              files = "\\.rs$";
              pass_filenames = false;
            };
            clippy = {
              enable = true;
              name = "cargo clippy";
              entry = "bash -c 'cd rust && cargo clippy --all-targets --all-features -- -D warnings'";
              files = "\\.(rs|toml)$";
              pass_filenames = false;
            };

            # Markdown formatting
            prettier = {
              enable = true;
              name = "prettier";
              entry = "${pkgs.nodePackages.prettier}/bin/prettier --write --prose-wrap always --print-width 100";
              types = ["markdown"];
            };
            markdownlint = {
              enable = true;
              name = "markdownlint";
              entry = "${pkgs.nodePackages.markdownlint-cli}/bin/markdownlint --fix";
              types = ["markdown"];
            };
          };
        };
      in {
        # Expose the pre-commit check as a flake check
        checks = {
          inherit pre-commit-check;
        };

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

                # Task runner
                just
              ];

              RUST_BACKTRACE = 1;

              # Inherit the shell hook from git-hooks to install pre-commit
              inherit (pre-commit-check) shellHook;
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
