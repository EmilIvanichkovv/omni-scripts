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
              ];
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
          local-git-branch-cleanup = pkgs.callPackage ./pkgs/local-git-branch-cleanup {
            inherit (pkgs) lib git;
            inherit (pkgs) rustPlatform;
          };
          default = self'.packages.local-git-branch-cleanup;
        };
      };
    };
}
