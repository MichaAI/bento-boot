# Загрузка в VM: initrd → система → передача экрана. Медленные юниты и
# один падающий — чтобы было на что посмотреть на скриншотах. Скриншоты
# каждые 10 с лежат в результате теста.
#   nix build .#checks.x86_64-linux.boot -L
{ pkgs, module }:

pkgs.testers.runNixOSTest {
  name = "bento-boot";
  nodes.machine =
    { config, pkgs, ... }:
    {
      imports = [ module ];
      michaai.bootScreen.enable = true;
      # Лиса печётся через Qt под Xvfb — для теста хватит планеты
      michaai.bootScreen.fox.enable = false;
      # KMS уже в initrd: иначе рисовать там не на чем
      # A real cryptsetup request before the root mount: exercise discovery,
      # rendering and keyboard input while only the initrd is available.
      boot.initrd.kernelModules = [ "aes_generic" "cbc" "bochs" "loop" "dm_crypt" ];
      boot.initrd.availableKernelModules = [ "aes_generic" "cbc" "xts" "sha256" "sha512" ];
      boot.initrd.systemd.storePaths = [ pkgs.cryptsetup pkgs.coreutils ];
      boot.initrd.systemd.services.bento-boot.serviceConfig.StandardError = "journal+console";
      boot.initrd.systemd.services.test-unlock = {
        wantedBy = [ "initrd.target" ];
        before = [ "sysroot.mount" ];
        after = [ "bento-boot.service" ];
        unitConfig.DefaultDependencies = false;
        serviceConfig = {
          Type = "oneshot";
          StandardOutput = "journal+console";
          StandardError = "journal+console";
        };
        script = ''
          ${pkgs.coreutils}/bin/truncate -s 32M /run/test-luks.img
          printf '%s' 'test-unlock' | ${pkgs.cryptsetup}/bin/cryptsetup luksFormat \
            --batch-mode --pbkdf pbkdf2 --pbkdf-force-iterations 1000 /run/test-luks.img -
          ${config.boot.initrd.systemd.package}/bin/systemd-cryptsetup attach testcrypt /run/test-luks.img - luks
          test -e /dev/mapper/testcrypt
          echo 'test-unlock: encrypted volume unlocked'
        '';
      };
      virtualisation.memorySize = 2048;
      # Экран выше 1080p, чтобы scale > 1: на нём рендер однажды паниковал,
      # пусть тест этот случай и держит
      virtualisation.qemu.options = [ "-device" "VGA,edid=on,xres=1920,yres=1200" ];

      systemd.services.slow-a = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig = {
          Type = "oneshot";
          ExecStart = "${pkgs.coreutils}/bin/sleep 25";
        };
      };
      systemd.services.slow-b = {
        wantedBy = [ "multi-user.target" ];
        after = [ "slow-a.service" ];
        serviceConfig = {
          Type = "oneshot";
          ExecStart = "${pkgs.coreutils}/bin/sleep 15";
        };
      };
      # Let the real-root tree and failures remain visible before handing
      # the display away; the actual module does not delay the greeter.
      systemd.services.bento-boot-quit.after = [ "slow-b.service" ];
      systemd.services.broken = {
        wantedBy = [ "multi-user.target" ];
        serviceConfig = {
          Type = "oneshot";
          ExecStart = "${pkgs.bash}/bin/sh -c 'echo cannot reach the thing; exit 3'";
        };
      };
    };

  testScript = ''
    import time
    machine.start()
    machine.wait_for_console_text("bento-boot: showing password request", timeout=90)
    time.sleep(2)
    machine.screenshot("initrd-password")
    machine.send_chars("test-unlock\n")
    machine.wait_for_console_text("test-unlock: encrypted volume unlocked", timeout=30)
    for i in range(5):
        time.sleep(10)
        machine.screenshot(f"boot{i:02}")
    machine.wait_for_unit("multi-user.target")
    machine.wait_until_succeeds("! systemctl is-active bento-boot.service")
    journal = machine.succeed("journalctl -b -u bento-boot.service -u bento-boot-quit.service --no-pager -o short-monotonic")
    print(journal)
    # Один процесс от initrd до конца: ни паники, ни перезапуска
    assert "panicked" not in journal, "bento-boot panicked"
    assert "Scheduled restart job" not in journal, "bento-boot was restarted"
    assert "followed PID 1 into the real root" in journal, "bento-boot did not follow the switch"
    assert "connected to PID 1 (initrd)" in journal, "bento-boot never reached the initrd's manager"
    assert "connected to PID 1 (real root)" in journal, "bento-boot never reached the real root's manager"
    initrd_journal, root_journal = journal.split("connected to PID 1 (real root)", 1)
    assert "manager snapshot:" in initrd_journal, "missing initrd job tree"
    assert "manager snapshot:" in root_journal, "missing real-root job tree"
    machine.succeed("systemctl show -p Result bento-boot.service | grep -q success")
    machine.screenshot("after")
  '';
}
