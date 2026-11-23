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

        git-manager = pkgs.callPackage ./local-git-branch-cleanup/tui.nix {
          inherit (pkgs)
            lib
            git
            sqlite
            pkg-config
            ;
          inherit craneLib;
        };

        # Old name: same package, plus the old command as a symlink so existing scripts and
        # `nix profile` installs keep working after the rename.
        local-git-branch-cleanup-tui = pkgs.symlinkJoin {
          inherit (self'.packages.git-manager) name meta;
          paths = [ self'.packages.git-manager ];
          postBuild = "ln -s git-manager $out/bin/local-git-branch-cleanup-tui";
        };
        default = self'.packages.git-manager;
      };
    };
}
