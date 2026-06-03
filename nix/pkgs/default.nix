{ inputs, ... }:
{
  perSystem =
    {
      pkgs,
      self',
      ...
    }:
    let
      craneLib = inputs.crane.mkLib pkgs;
    in
    {
      packages = {
        # Bash script version (original)
        local-git-branch-cleanup = pkgs.callPackage ./local-git-branch-cleanup { };

        # Rust TUI version (interactive)
        local-git-branch-cleanup-tui = pkgs.callPackage ./local-git-branch-cleanup/tui.nix {
          inherit (pkgs) lib git sqlite pkg-config;
          inherit craneLib;
        };

        # Default to TUI version
        default = self'.packages.local-git-branch-cleanup-tui;
      };
    };
}
