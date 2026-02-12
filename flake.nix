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
              ];
              
              RUST_BACKTRACE = 1;
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
