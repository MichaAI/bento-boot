//! bento-boot: a boot screen that shows what systemd is doing as orbits
//! around a dithered fox, and asks for disk passphrases.
//!
//! ```text
//! bento-boot [run] [--config PATH]   the boot screen (as root, from the unit)
//! bento-boot quit [--retain]         fade it out and wait until it is gone
//! bento-boot restore-console         give the text console back (ExecStopPost)
//! bento-boot preview [--out DIR] [--size WxH] [--scale F] [--at S,S,…] [--apng FPS]
//! ```

#![cfg_attr(not(feature = "kms"), allow(dead_code))]

mod color;
mod config;
mod console;
mod control;
mod controller;
mod fox;
mod logs;
mod preview;
mod scene;
mod scripted;
mod text;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

slint::include_modules!();

/// Exit code asking systemd to start us again: the DRM device changed
/// under us (the firmware framebuffer gave way to the real GPU driver).
const EXIT_RESTART: u8 = 75;

fn usage() -> ExitCode {
    eprintln!(
        "usage: bento-boot [run] [--config PATH]\n       bento-boot quit [--retain]\n       \
         bento-boot restore-console\n       bento-boot preview [--out DIR] [--size WxH] [--scale F] [--at S,S,...] [--apng FPS] [--config PATH]"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.first().map(String::as_str) {
        Some(c @ ("run" | "quit" | "restore-console" | "preview")) => (c, &args[1..]),
        Some("-h" | "--help") => return usage(),
        _ => ("run", &args[..]),
    };
    let mut config_path = PathBuf::from(config::DEFAULT_PATH);
    let mut retain = false;
    let mut out = PathBuf::from("preview");
    let mut size = (1920u32, 1080u32);
    let mut scale = 1.0f32;
    let mut stills = vec![0.8, 2.6, 5.0, 9.0, 11.9, 13.3, 16.2];
    let mut apng = 0u32;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().cloned().unwrap_or_default();
        match a.as_str() {
            "--config" => config_path = PathBuf::from(val()),
            "--retain" => retain = true,
            "--out" => out = PathBuf::from(val()),
            "--size" => {
                let v = val();
                let Some((w, h)) = v.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))) else {
                    return usage();
                };
                size = (w, h);
            }
            "--scale" => scale = val().parse().unwrap_or(1.0),
            "--at" => stills = val().split(',').filter_map(|s| s.trim().parse().ok()).collect(),
            "--apng" => apng = val().parse().unwrap_or(0),
            _ => return usage(),
        }
    }

    match cmd {
        "quit" => match control::quit(retain, Duration::from_secs(5)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("bento-boot quit: {e}");
                ExitCode::FAILURE
            }
        },
        "restore-console" => {
            let _ = console::restore();
            ExitCode::SUCCESS
        }
        "preview" => {
            let (config, err) = config::Config::load(&config_path);
            if let Some(e) = err {
                eprintln!("bento-boot: {e}");
            }
            let opts = preview::Options { out, width: size.0, height: size.1, scale, stills, apng_fps: apng, config };
            match preview::run(opts) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("bento-boot preview: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        _ => run(&config_path),
    }
}

#[cfg(not(feature = "kms"))]
fn run(_: &std::path::Path) -> ExitCode {
    eprintln!("bento-boot: built without the kms feature; only `preview` works");
    ExitCode::FAILURE
}

/// Names of the DRM cards, to notice the firmware framebuffer handing over
/// to the real driver.
#[cfg(feature = "kms")]
fn drm_cards() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir("/sys/class/drm")
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.starts_with("card") && !n.contains('-'))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[cfg(feature = "kms")]
fn run(config_path: &std::path::Path) -> ExitCode {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::mpsc;

    use bento_boot_secret::agent::{Agent, monotonic_usec};
    use slint::ComponentHandle;

    if !bento_boot_systemd::in_initrd() && bento_boot_systemd::startup_finished() == Some(true) {
        eprintln!("bento-boot: the boot is already over, nothing to show");
        return ExitCode::SUCCESS;
    }
    let (cfg, cfg_err) = config::Config::load(config_path);
    if let Err(e) = bento_boot_secret::memory::harden_process() {
        eprintln!("bento-boot: no core-dump protection: {e}");
    }
    let console = console::Console::take().map_err(|e| eprintln!("bento-boot: console stays as is: {e}")).ok();
    let control = control::Control::bind().map_err(|e| eprintln!("bento-boot: no control socket: {e}")).ok();

    let (tx, rx) = mpsc::channel();
    let follower = std::thread::Builder::new().name("systemd".into()).spawn(move || bento_boot_systemd::follow(&tx));
    if let Err(e) = follower {
        eprintln!("bento-boot: cannot follow systemd: {e}");
    }

    if let Err(e) =
        slint::BackendSelector::new().backend_name("linuxkms".into()).renderer_name("software".into()).select()
    {
        eprintln!("bento-boot: no display: {e}");
        return ExitCode::FAILURE;
    }
    let app = match App::new() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("bento-boot: {e}");
            return ExitCode::FAILURE;
        }
    };

    let scale_cfg = cfg.screen.scale;
    let ctl = Rc::new(RefCell::new(controller::Controller::new(
        cfg,
        Box::new(Agent::system()),
        controller::SystemInfo::read,
    )));
    ctl.borrow_mut().set_note(cfg_err);
    ctl.borrow_mut().set_log_feed(logs::LogFeed::start());
    {
        let ctl = ctl.clone();
        app.on_key(move |t, control| ctl.borrow_mut().key(&t, control, monotonic_usec()));
    }

    let quit_retain = Rc::new(RefCell::new(None::<bool>));
    let restart = Rc::new(RefCell::new(false));
    let control = Rc::new(RefCell::new(control));
    let cards = drm_cards();
    let mut last_card_check = 0u64;

    let timer = slint::Timer::default();
    {
        let app = app.as_weak();
        let ctl = ctl.clone();
        let quit_retain = quit_retain.clone();
        let restart = restart.clone();
        let control = control.clone();
        let mut scale_sent = false;
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || {
            let Some(app) = app.upgrade() else { return };
            let now = monotonic_usec();
            let mut c = ctl.borrow_mut();
            while let Ok(m) = rx.try_recv() {
                c.feed(m, now);
            }
            if let Some(req) = control.borrow_mut().as_mut().and_then(control::Control::poll) {
                *quit_retain.borrow_mut() = Some(req.retain);
                c.leave();
            }
            if now.saturating_sub(last_card_check) > 500_000 {
                last_card_check = now;
                // Right after the switch from the initrd, until we follow
                // PID 1 into the new root, our /sys is the old, empty one:
                // no cards is not a different card.
                let now_cards = drm_cards();
                if !now_cards.is_empty() && now_cards != cards {
                    *restart.borrow_mut() = true;
                    c.leave();
                }
            }
            let phys = app.window().size();
            let scale = if scale_cfg > 0.0 { scale_cfg } else { (phys.height as f32 / 1080.0).max(1.0) };
            if !scale_sent {
                app.window().dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: scale });
                scale_sent = true;
            }
            let size = (phys.width as f32 / scale, phys.height as f32 / scale);
            if c.tick(&app, now, size, scale) == controller::Next::Quit {
                let _ = slint::quit_event_loop();
            }
        });
    }

    if let Err(e) = app.show() {
        eprintln!("bento-boot: {e}");
        return ExitCode::FAILURE;
    }
    if let Err(e) = slint::run_event_loop() {
        eprintln!("bento-boot: {e}");
    }
    drop(timer);
    let _ = app.hide();
    drop(app);

    let retain = quit_retain.borrow().unwrap_or(false);
    if let Some(console) = console {
        console.release(retain);
    }
    if let Some(control) = control.borrow_mut().take() {
        control.finish();
    }
    if *restart.borrow() { ExitCode::from(EXIT_RESTART) } else { ExitCode::SUCCESS }
}
