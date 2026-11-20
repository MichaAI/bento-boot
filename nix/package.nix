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
  python3,
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

  # Software alpha blending must read cached RAM, not the GPU's mapped
  # scanout buffer. Patch the writable vendor copy, keeping Cargo's file
  # checksums accurate; the locked upstream crate stays unchanged.
  postPatch = ''
    vendor="$cargoDepsCopy/i-slint-backend-linuxkms-1.19.0"
    patch -d "$vendor" -p1 < ${./patches/slint-kms-shadow-buffer.patch}
    ${python3}/bin/python - "$vendor" <<'PY'
    import hashlib, json, pathlib, sys
    crate = pathlib.Path(sys.argv[1])
    checksums = crate / ".cargo-checksum.json"
    data = json.loads(checksums.read_text())
    data["files"]["renderer/sw.rs"] = hashlib.sha256((crate / "renderer/sw.rs").read_bytes()).hexdigest()
    checksums.write_text(json.dumps(data))
    PY
  '';

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
