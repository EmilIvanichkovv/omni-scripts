{
  lib,
  stdenvNoCC,
  makeWrapper,
  git,
  bash,
}:

stdenvNoCC.mkDerivation rec {
  pname = "local-git-branch-cleanup";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../../../bash;
    fileset = lib.fileset.unions [
      ../../../bash/local-git-branch-cleanup.sh
      ../../../bash/utils
    ];
  };

  nativeBuildInputs = [ makeWrapper ];

  installPhase = ''
    runHook preInstall

    # Install the script and utils
    mkdir -p $out/libexec/local-git-branch-cleanup
    cp -r . $out/libexec/local-git-branch-cleanup/

    # Create wrapper script in bin
    mkdir -p $out/bin
    makeWrapper $out/libexec/local-git-branch-cleanup/local-git-branch-cleanup.sh \
      $out/bin/local-git-branch-cleanup \
      --prefix PATH : ${lib.makeBinPath [ git bash ]}

    runHook postInstall
  '';

  meta = with lib; {
    description = "Bash script for cleaning up local Git branches without remote counterparts";
    homepage = "https://github.com/EmilIvanichkovv/omni-scripts";
    license = licenses.mit;
    maintainers = [ ];
    mainProgram = "local-git-branch-cleanup";
    platforms = platforms.unix;
  };
}
