{ pkgs, stdenv, ... }:
stdenv.mkDerivation {
  pname = "local-git-branch-cleanup";
  version = "0.1";
  src = ../../bash;
  buildPhase = ''
    mkdir -p $out/bin
    cp -r $src/* $out/bin
    mv $out/bin/local-git-branch-cleanup.sh $out/bin/local-git-branch-cleanup
  '';
}
