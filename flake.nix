{
  description = "bento-boot: экран загрузки — юниты systemd орбитами вокруг дизеринговой лисы";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      nixosModules.default = import ./nix/nixos.nix;

      packages = forAll (pkgs: {
        default = pkgs.callPackage ./nix/package.nix { };
        # Кадры лисы: .override { model = ./Fox.glb; animation = "Walk"; … }
        fox = pkgs.callPackage ./nix/fox.nix { };
      });

      checks.x86_64-linux.boot = import ./nix/vm-test.nix {
        pkgs = nixpkgs.legacyPackages.x86_64-linux;
        module = self.nixosModules.default;
      };

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            pkg-config
            libinput
            udev
            libxkbcommon
            fontconfig
            freetype
          ];
        };
      });

      formatter = forAll (pkgs: pkgs.nixfmt);
    };
}
