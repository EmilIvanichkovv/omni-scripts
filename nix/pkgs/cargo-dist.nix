# dist pinned to the version recorded in rust/dist-workspace.toml. nixpkgs in flake.lock lags
# behind, and the generated release workflow must be regenerated with the exact same version, so
# this uses the upstream static musl binary instead of the nixpkgs build.
{
  lib,
  stdenvNoCC,
  fetchurl,
}:

stdenvNoCC.mkDerivation (finalAttrs: {
  pname = "cargo-dist";
  version = "0.33.0";

  src = fetchurl {
    url = "https://github.com/axodotdev/cargo-dist/releases/download/v${finalAttrs.version}/cargo-dist-x86_64-unknown-linux-musl.tar.xz";
    hash = "sha256-uOlbx2xjN1lYFz71ri29jpIRzB7QPN7giZcCdmtMKi4=";
  };

  installPhase = ''
    runHook preInstall
    install -Dm755 dist $out/bin/dist
    runHook postInstall
  '';

  meta = {
    description = "Shippable application packaging (release workflow generator)";
    homepage = "https://github.com/axodotdev/cargo-dist";
    license = with lib.licenses; [
      mit
      asl20
    ];
    platforms = [ "x86_64-linux" ];
    mainProgram = "dist";
  };
})
