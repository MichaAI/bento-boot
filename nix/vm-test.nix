# Загрузка в VM: initrd → система → передача экрана. Медленные юниты и
# один падающий — чтобы было на что посмотреть на скриншотах. Скриншоты
# каждые 10 с лежат в результате теста.
#   nix build .#checks.x86_64-linux.boot -L
{ pkgs, module }:

pkgs.testers.runNixOSTest {
  name = "bento-boot";
  nodes.machine =
    { pkgs, ... }:
    {
      imports = [ module ];
      michaai.bootScreen.enable = true;
      # Лиса печётся через Qt под Xvfb — для теста хватит планеты
      michaai.bootScreen.fox.enable = false;
      # KMS уже в initrd: иначе рисовать там не на чем
      boot.initrd.kernelModules = [ "bochs" ];
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
    for i in range(14):
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
    machine.succeed("systemctl show -p Result bento-boot.service | grep -q success")
    machine.screenshot("after")
  '';
}
