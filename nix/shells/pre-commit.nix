{ inputs, ... }:
{
  perSystem =
    {
      pkgs,
      system,
      ...
    }:
    let
      pre-commit-check = inputs.git-hooks.lib.${system}.run {
        src = ../..;
        hooks = {
          # General hooks
          trailing-whitespace = {
            enable = true;
            name = "trim trailing whitespace";
            entry = "${pkgs.python3Packages.pre-commit-hooks}/bin/trailing-whitespace-fixer";
            types = [ "text" ];
          };
          end-of-file-fixer = {
            enable = true;
            name = "fix end of files";
            entry = "${pkgs.python3Packages.pre-commit-hooks}/bin/end-of-file-fixer";
            types = [ "text" ];
          };
          check-yaml = {
            enable = true;
            name = "check yaml";
            entry = "${pkgs.python3Packages.pre-commit-hooks}/bin/check-yaml";
            types = [ "yaml" ];
          };
          check-toml = {
            enable = true;
            name = "check toml";
            entry = "${pkgs.python3Packages.pre-commit-hooks}/bin/check-toml";
            types = [ "toml" ];
          };
          check-merge-conflict = {
            enable = true;
            name = "check for merge conflicts";
            entry = "${pkgs.python3Packages.pre-commit-hooks}/bin/check-merge-conflict";
            types = [ "text" ];
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
            types = [ "markdown" ];
          };
          markdownlint = {
            enable = true;
            name = "markdownlint";
            entry = "${pkgs.nodePackages.markdownlint-cli}/bin/markdownlint --fix";
            types = [ "markdown" ];
          };
        };
      };
    in
    {
      _module.args.pre-commit-check = pre-commit-check;

      checks = {
        inherit pre-commit-check;
      };
    };
}
