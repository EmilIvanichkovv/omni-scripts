{ ... }:
{
  imports = [
    ./pre-commit.nix
  ];

  perSystem =
    {
      pkgs,
      pre-commit-check,
      ...
    }:
    {
      devShells = {
        default = pkgs.mkShell {
          packages = with pkgs; [
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

          inherit (pre-commit-check) shellHook;
        };

        rust-tui = pkgs.mkShell {
          buildInputs = with pkgs; [
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
    };
}
