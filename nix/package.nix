# bento-boot: экран загрузки на Slint (linuxkms, программный рендер)
{
  lib,
  rustPlatform,
  pkg-config,
  libinput,
  udev,
  libxkbcommon,
  fontconfig,
  callPackage,
}:

let
  fonts = callPackage ./fonts.nix { };
in
rustPlatform.buildRustPackage {
  pname = "bento-boot";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
    ];
  };

  cargoLock.lockFile = ../Cargo.lock;
  cargoBuildFlags = [ "-p" "bento-boot" ];
  cargoTestFlags = [ "--workspace" ];

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [
    libinput
    udev
    libxkbcommon
    fontconfig
  ];

  env.BENTO_FONT_DIR = fonts;

  passthru = { inherit fonts; };

  meta = {
    description = "Экран загрузки: юниты systemd орбитами вокруг лисы, пароль LUKS точками";
    license = with lib.licenses; [
      mit
      asl20
    ];
    mainProgram = "bento-boot";
    platforms = lib.platforms.linux;
  };
}
