{
  nixConfig = {
    extra-substituters = [ "https://git-branch-manager.cachix.org" ];
    extra-trusted-public-keys = [
      "git-branch-manager.cachix.org-1:Cp9s0Krvz/Q9GXq0ZPfIsW473/tNIreMOZApP5LuHgE="
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    crane.url = "github:ipetkov/crane";
  };

  outputs =
    inputs@{ flake-parts, crane, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } (
      { lib, ... }:
      {
        imports = [
          (lib.path.append ./. "nix")
        ];

        systems = [ "x86_64-linux" ];
      }
    );
}
