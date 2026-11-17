# Шрифты экрана: Nunito статичными начертаниями (софтверный рендер Slint
# берёт у вариативного шрифта только одно) и JetBrains Mono для журнала
{
  runCommand,
  nunito,
  jetbrains-mono,
  python3Packages,
}:

runCommand "bento-boot-fonts" { nativeBuildInputs = [ python3Packages.fonttools ]; } ''
  mkdir -p $out
  src="${nunito}/share/fonts/truetype/Nunito/Nunito[wght].ttf"
  for w in 400:Regular 700:Bold 800:ExtraBold; do
    fonttools varLib.instancer "$src" wght=''${w%%:*} --update-name-table -q \
      -o $out/Nunito-''${w##*:}.ttf
  done
  cp ${jetbrains-mono}/share/fonts/truetype/JetBrainsMono-Regular.ttf $out/
''
