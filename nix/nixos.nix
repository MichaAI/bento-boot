# Модуль NixOS: michaai.bootScreen — экран загрузки bento-boot вместо
# Plymouth, с первых секунд initrd и до передачи экрана SDDM
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.michaai.bootScreen;
  inherit (lib) mkOption mkEnableOption types;

  package = pkgs.callPackage ./package.nix { };
  frames =
    if cfg.fox.enable then
      pkgs.callPackage ./fox.nix {
        inherit (cfg.fox)
          model
          animation
          speed
          seconds
          fps
          size
          modelSize
          ;
      }
    else
      null;

  configFile = (pkgs.formats.toml { }).generate "bento-boot.toml" {
    colors = cfg.colors;
    screen = cfg.screen;
    fox = {
      inherit (cfg.fox) fps size pixel;
    }
    // lib.optionalAttrs (cfg.fox.palette != null) { inherit (cfg.fox) palette; }
    // lib.optionalAttrs (frames != null) { frames = "${frames}"; };
  };

  hasDisplayManager = config.services.displayManager.enable || config.services.xserver.enable;

  # Один и тот же юнит в initrd и в системе: после переезда в настоящий
  # корень systemd узнаёт в нём уже запущенный и не стартует второй раз
  unit = {
    description = "Boot screen";
    wantedBy = [ "sysinit.target" ];
    after = [ "systemd-udevd.service" ];
    unitConfig = {
      DefaultDependencies = false;
      # Переход в initrd-switch-root.target его не останавливает
      IgnoreOnIsolate = true;
    };
    environment = {
      XKB_DEFAULT_LAYOUT = cfg.keyboardLayout;
    };
    serviceConfig = {
      # «@» в argv[0]: так systemd не добивает процесс при переезде
      ExecStart = "@${package}/bin/bento-boot @bento-boot run";
      ExecStopPost = "${package}/bin/bento-boot restore-console";
      # Перезапуск только с кодом 75 — видеокарта сменилась под нами
      # (simpledrm → настоящий драйвер). Остальные падения не рестартуем:
      # crash-loop выхватывает консоль и DRM-master из-под запускающегося
      # дисплей-менеджера
      Restart = "no";
      RestartForceExitStatus = "75";
      RestartSec = 0;
      NoNewPrivileges = true;
      LockPersonality = true;
      RestrictRealtime = true;
      RestrictNamespaces = true;
      MemoryDenyWriteExecute = true;
      SystemCallArchitectures = "native";
      RestrictAddressFamilies = [
        "AF_UNIX"
        "AF_NETLINK"
      ];
    };
  };
in
{
  options.michaai.bootScreen = {
    enable = mkEnableOption "экран загрузки bento-boot (вместо Plymouth)";

    package = mkOption {
      type = types.package;
      readOnly = true;
      default = package;
      description = "Собранный bento-boot.";
    };

    keyboardLayout = mkOption {
      type = types.str;
      default = "us";
      description = ''
        Раскладка для пароля диска (XKB). Пароль от LUKS обычно набирают
        в той же раскладке, что и в консоли, то есть в us.
      '';
    };

    quiet = mkOption {
      type = types.bool;
      default = true;
      description = "Убрать строки ядра и systemd с консоли под экраном.";
    };

    colors = mkOption {
      type = types.attrsOf types.str;
      default = { };
      example = {
        accent = "#89b4fa";
        accent2 = "#f5c2e7";
      };
      description = ''
        Цвета поверх Catppuccin Mocha: base, surface, rings, overlay, subtext,
        text, accent, accent2, red, green, yellow.
      '';
    };

    screen = mkOption {
      type = types.attrsOf (
        types.oneOf [
          types.bool
          types.int
          types.float
        ]
      );
      default = { };
      example = {
        tiles = false;
        bounce = 0.8;
      };
      description = ''
        Секция [screen] конфига: scale (0 — по высоте экрана), dither,
        tiles, hide_runs_shorter_than (мс), linger (мс), bounce (0…1).
      '';
    };

    fox = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = "Лиса в центре; без неё — дизеринговая планета.";
      };
      model = mkOption {
        type = types.nullOr types.path;
        default = null;
        example = lib.literalExpression "./Fox.glb";
        description = "3D-модель (.glb/.gltf/.fbx/.obj); null — лиса Khronos.";
      };
      animation = mkOption {
        type = types.str;
        default = "Run";
        description = ''Анимация из модели; "" — модель стоит и просто крутится.'';
      };
      speed = mkOption {
        type = types.number;
        default = 1;
        description = "Скорость анимации: подгоняется под целое число повторов за оборот.";
      };
      seconds = mkOption {
        type = types.number;
        default = 4;
        description = "Один полный оборот, секунды.";
      };
      fps = mkOption {
        type = types.ints.positive;
        default = 30;
        description = "Кадров в секунду. Кадры лежат в initrd: больше кадров — толще initrd.";
      };
      size = mkOption {
        type = types.ints.positive;
        default = 320;
        description = "Сторона кадра, px; на экране 1080p — один к одному.";
      };
      modelSize = mkOption {
        type = types.number;
        default = 280;
        description = "Размер модели в кадре, px.";
      };
      pixel = mkOption {
        type = types.number;
        default = 4;
        description = "Клетка дизеринга лисы, px кадра.";
      };
      palette = mkOption {
        type = types.nullOr (types.listOf types.str);
        default = null;
        description = "4 цвета модели от тёмного к светлому; null — из цветов темы.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = !config.boot.plymouth.enable;
        message = "michaai.bootScreen заменяет Plymouth: выключи boot.plymouth.enable.";
      }
    ];

    environment.etc."bento-boot.toml".source = configFile;
    environment.systemPackages = [ package ];

    boot.initrd = {
      # evdev — чтобы libinput видел клавиатуру для пароля
      kernelModules = [ "evdev" ];
      systemd = {
        services.bento-boot = unit;
        contents."/etc/bento-boot.toml".source = configFile;
        storePaths = [
          package
          configFile
          pkgs.xkeyboard_config
          # Кварки устройств libinput лежат рядом с библиотекой, в share/
          pkgs.libinput.out
        ]
        ++ lib.optional (frames != null) frames;
      };
    };

    systemd.services.bento-boot = unit // {
      # nixos-rebuild switch не должен вытаскивать экран загрузки поверх
      # рабочего стола; а если всё же стартует — увидит, что загрузка
      # закончилась, и выйдет сразу
      restartIfChanged = false;
      stopIfChanged = false;
    };

    # Как plymouth-quit: экран уходит перед входом в систему
    systemd.services.bento-boot-quit = {
      description = "Hand the screen over from the boot screen";
      wantedBy = [ "multi-user.target" ];
      after = [
        "bento-boot.service"
        "systemd-user-sessions.service"
      ];
      before = [
        "display-manager.service"
        "getty@tty1.service"
      ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${package}/bin/bento-boot quit" + lib.optionalString hasDisplayManager " --retain";
        TimeoutStartSec = 10;
      };
    };

    boot.consoleLogLevel = lib.mkIf cfg.quiet (lib.mkDefault 3);
    boot.kernelParams = lib.mkIf cfg.quiet [
      "quiet"
      "udev.log_level=3"
      "systemd.show_status=auto"
    ];
  };
}
