import QtQuick
import QtQuick.Window

// Рисует полный оборот модели кадрами frame-<i>.png и выходит. Оборот
// зациклен без шва: последний кадр переходит в первый и по повороту, и по
// анимации. Кадры — обычный свет и цвет модели на прозрачном фоне; в
// четыре цвета темы их дизерит bento-boot при показе.
// Параметры — JSON в последнем аргументе (см. nix/fox.nix)
Window {
    id: win

    readonly property var p: JSON.parse(Qt.application.arguments[Qt.application.arguments.length - 1])
    readonly property int frames: p.frames

    width: p.size
    height: p.size
    visible: true
    color: "transparent"

    Avatar {
        id: avatar
        width: win.p.size
        height: win.p.size
        scene: win.p.scene
        animation: win.p.animation
        modelSize: win.p.modelSize
    }

    function save(item, name, next) {
        item.grabToImage(r => {
            if (!r.saveToFile(p.out + "/" + name)) {
                console.error("render: не записался", name)
                Qt.exit(1)
            }
            next()
        })
    }

    // Целое число повторов анимации на один оборот — иначе на стыке шов.
    // Скорость при этом чуть подгоняется под длину оборота
    readonly property int loops: avatar.duration > 0
        ? Math.max(1, Math.round(p.seconds * 1000 * p.speed / avatar.duration))
        : 0

    function frame(i) {
        if (i === frames) {
            console.log("render:", frames, "кадров, анимация",
                        p.animation || "нет", loops ? "× " + loops + " за оборот" : "")
            Qt.quit()
            return
        }
        avatar.angle = 360 * i / frames
        avatar.time = avatar.duration * loops * i / frames
        save(avatar, "frame-" + i + ".png", () => frame(i + 1))
    }

    // Ждём, пока сцена загрузится и меши посчитают габариты
    Timer {
        interval: 100
        repeat: true
        running: true
        property int waited: 0
        onTriggered: {
            if (avatar.ready) {
                stop()
                win.frame(0)
            } else if (++waited > 300) {
                console.error("render: сцена так и не загрузилась")
                Qt.exit(1)
            }
        }
    }
}
