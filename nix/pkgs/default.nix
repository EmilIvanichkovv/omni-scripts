{ ... }:
{
  perSystem =
    {
      pkgs,
      self',
      ...
    }:
    {
      packages = {
        # Bash script version (original)
        local-git-branch-cleanup = pkgs.callPackage ./local-git-branch-cleanup { };

        # Rust TUI version (interactive)
        local-git-branch-cleanup-tui = pkgs.callPackage ./local-git-branch-cleanup/tui.nix {
          inherit (pkgs) lib git sqlite;
          inherit (pkgs) rustPlatform;
        };

        # Default to TUI version
        default = self'.packages.local-git-branch-cleanup-tui;
      };
    };
}
