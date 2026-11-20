//! Glue: systemd's messages and keystrokes in, a frame of properties out.
//! The same code drives the real screen and `bento-boot preview`; only the
//! clock and the source of password requests differ.

use std::collections::{HashMap, VecDeque};
use std::io;

use bento_boot_model::{Boot, JobState, Stage, Usec};
use bento_boot_secret::Passphrase;
use bento_boot_secret::agent::{self, Change, Request};
use bento_boot_secret::prompt::{Key, Outcome};
use bento_boot_systemd::Message;
use slint::{ComponentHandle, Image, ModelRc, SharedString, VecModel};
use tiny_skia::Pixmap;

use crate::color::Palette;
use crate::config::Config;
use crate::fox::Fox;
use crate::logs::LogFeed;
use crate::scene::{Geometry, Scene};
use crate::{App, Label, Row, text};

/// Where password requests come from and where answers go.
pub trait Asker {
    fn scan(&mut self) -> Vec<Change>;
    fn send(&mut self, req: &Request, datagram: &[u8]) -> io::Result<()>;
}

impl Asker for agent::Agent {
    fn scan(&mut self) -> Vec<Change> {
        agent::Agent::scan(self)
    }

    fn send(&mut self, req: &Request, datagram: &[u8]) -> io::Result<()> {
        agent::send(req, datagram)
    }
}

struct Asking {
    req: Request,
    prompt: Box<Passphrase>,
}

/// Host, system and kernel, re-read after the switch to the real root.
#[derive(Default)]
pub struct SystemInfo {
    pub host: String,
    pub os: String,
}

impl SystemInfo {
    #[must_use]
    pub fn read() -> Self {
        let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default().trim().to_owned();
        let release = ["/etc/os-release", "/usr/lib/os-release", "/etc/initrd-release"]
            .iter()
            .find_map(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let pretty = release
            .lines()
            .find_map(|l| l.strip_prefix("PRETTY_NAME="))
            .map(|v| v.trim_matches('"').to_owned())
            .unwrap_or_else(|| "Linux".to_owned());
        let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim().to_owned();
        let host =
            if host.is_empty() || host == "(none)" || host == "localhost" { "привет".to_owned() } else { host };
        Self { host, os: if kernel.is_empty() { pretty } else { format!("{pretty} · {kernel}") } }
    }
}

pub struct Controller {
    cfg: Config,
    pal: Palette,
    pub boot: Boot,
    scene: Scene,
    fox: Fox,
    info: SystemInfo,
    read_info: fn() -> SystemInfo,
    causes: HashMap<String, String>,
    asker: Box<dyn Asker>,
    asking: Option<Asking>,
    queue: VecDeque<Request>,
    /// Id and time of the last passphrase sent, to tell a wrong one when
    /// the same request comes back.
    submitted: Option<(String, Usec)>,
    hint: String,
    shake_until: Usec,
    ask_alpha: f32,
    last: Option<Usec>,
    last_scan: Usec,
    finished_at: Option<Usec>,
    leaving: bool,
    presence: f32,
    /// A problem with the config file, shown for the whole boot.
    note: Option<String>,
    /// systemd not answering; cleared as soon as it does.
    link_note: Option<String>,
    pix: Option<Pixmap>,
    seen_stage: Stage,
    /// The unit's last journal lines, by unit.
    unit_logs: HashMap<String, Vec<String>>,
    drawer_open: bool,
    drawer: f32,
    drawer_lines: Vec<String>,
    log_feed: Option<LogFeed>,
}

/// What the caller should do after a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    Continue,
    /// Faded out: stop the event loop.
    Quit,
}

impl Controller {
    #[must_use]
    pub fn new(cfg: Config, asker: Box<dyn Asker>, read_info: fn() -> SystemInfo) -> Self {
        let pal = Palette::from_config(&cfg.colors);
        let fox_palette = crate::fox::palette(&cfg.fox.palette, &pal);
        let fox = Fox::load(cfg.fox.frames.as_deref(), cfg.fox.fps, cfg.fox.pixel, Some(fox_palette));
        Self {
            pal,
            fox,
            boot: Boot::new(),
            scene: Scene::default(),
            info: read_info(),
            read_info,
            causes: HashMap::new(),
            asker,
            asking: None,
            queue: VecDeque::new(),
            submitted: None,
            hint: String::new(),
            shake_until: 0,
            ask_alpha: 0.0,
            last: None,
            last_scan: 0,
            finished_at: None,
            leaving: false,
            presence: 0.0,
            note: None,
            link_note: None,
            pix: None,
            seen_stage: Stage::Connecting,
            unit_logs: HashMap::new(),
            drawer_open: false,
            drawer: 0.0,
            drawer_lines: Vec::new(),
            log_feed: None,
            cfg,
        }
    }

    /// Where the log drawer gets its lines; without one it shows what
    /// [`Controller::set_drawer_lines`] gave it.
    pub fn set_log_feed(&mut self, feed: Option<LogFeed>) {
        self.log_feed = feed;
    }

    pub fn set_drawer_lines(&mut self, lines: Vec<String>) {
        self.drawer_lines = lines;
    }

    pub fn toggle_drawer(&mut self) {
        self.drawer_open = !self.drawer_open;
        if let Some(f) = &self.log_feed {
            f.set_open(self.drawer_open);
        }
    }

    pub fn set_note(&mut self, note: Option<String>) {
        self.note = note;
    }

    /// Requests already seen before the window was created, e.g. after a
    /// DRM-driven process restart while systemd still awaits a passphrase.
    pub fn seed_asks(&mut self, changes: Vec<Change>, now: Usec) {
        for change in changes {
            if let Change::Asked(req) = change {
                if self.asking.is_none() {
                    self.start_asking(req, now);
                } else {
                    self.queue.push_back(req);
                }
            }
        }
    }

    pub fn feed(&mut self, msg: Message, now: Usec) {
        match msg {
            Message::Boot(e) => {
                // systemd answers again: whatever we said about it is stale.
                if matches!(e, bento_boot_model::Event::Stage(_)) {
                    self.link_note = None;
                }
                self.boot.apply(e, now);
            }
            Message::Cause { unit, cause } => {
                let reason = text::cause(&cause);
                self.causes.insert(unit.clone(), reason.clone());
                self.boot.apply(bento_boot_model::Event::FailureReason { unit, reason }, now);
            }
            Message::Journal { unit, lines } => {
                self.unit_logs.insert(unit, lines);
            }
            Message::Unreachable(why) => self.link_note = Some(format!("systemd молчит: {why}")),
        }
        if self.boot.stage != self.seen_stage {
            self.seen_stage = self.boot.stage;
            if matches!(self.boot.stage, Stage::Main | Stage::Finished) {
                self.info = (self.read_info)();
            }
        }
    }

    /// Fade out and stop, e.g. when the display manager wants the screen.
    pub fn leave(&mut self) {
        self.leaving = true;
    }

    pub fn save_handoff(&self, app: &App) -> io::Result<()> {
        let Some(frame) = self.fox.frame() else { return Ok(()) };
        let size = app.window().size();
        let scale = app.window().scale_factor();
        crate::handoff::save(
            frame,
            app.get_center_x() * scale / size.width.max(1) as f32,
            app.get_center_y() * scale / size.height.max(1) as f32,
            app.get_fox_size() * scale / size.height.max(1) as f32,
        )
    }

    pub fn key(&mut self, text: &str, control: bool, now: Usec) {
        let f2 = slint::SharedString::from(slint::platform::Key::F2);
        if self.asking.is_none() && (text == f2.as_str() || text == "\t") {
            self.toggle_drawer();
            return;
        }
        let Some(a) = self.asking.as_mut() else { return };
        let keys: Vec<Key> = match text {
            "\u{8}" | "\u{7f}" => vec![Key::Backspace],
            "\n" | "\r" => vec![Key::Enter],
            "\u{1b}" => vec![Key::Escape],
            "u" | "U" if control => vec![Key::ClearLine],
            _ if control => Vec::new(),
            t => t.chars().map(Key::Char).collect(),
        };
        for k in keys {
            match a.prompt.key(k) {
                Outcome::Submit => {
                    let r = self.asker.send(&a.req, a.prompt.reply().unwrap_or(b"-"));
                    a.prompt.wipe();
                    self.submitted = Some((a.req.id.clone(), now));
                    self.hint = if r.is_ok() {
                        "проверяю…".to_owned()
                    } else {
                        "не отправилось".to_owned()
                    };
                }
                Outcome::Cancel => {
                    let _ = self.asker.send(&a.req, b"-");
                    self.asking = None;
                    self.next_request(now);
                    return;
                }
                Outcome::Rejected => self.shake_until = now + 350_000,
                Outcome::Changed => self.hint.clear(),
                Outcome::Ignored => {}
            }
        }
    }

    fn start_asking(&mut self, req: Request, now: Usec) {
        eprintln!("bento-boot: showing password request {:?}", req.file);
        let wrong =
            self.submitted.as_ref().is_some_and(|(id, at)| *id == req.id && now.saturating_sub(*at) < 60_000_000);
        self.hint = if wrong { "не подошло, давай ещё раз".to_owned() } else { String::new() };
        if wrong {
            self.shake_until = now + 450_000;
        }
        let prompt = Box::new(Passphrase::new());
        let _ = bento_boot_secret::memory::lock(prompt.buffer().storage());
        self.asking = Some(Asking { req, prompt });
    }

    fn next_request(&mut self, now: Usec) {
        if let Some(r) = self.queue.pop_front() {
            self.start_asking(r, now);
        }
    }

    fn poll_asks(&mut self, now: Usec) {
        if now.saturating_sub(self.last_scan) < 150_000 {
            return;
        }
        self.last_scan = now;
        for change in self.asker.scan() {
            match change {
                Change::Asked(req) => {
                    if self.asking.is_none() {
                        self.start_asking(req, now);
                    } else {
                        self.queue.push_back(req);
                    }
                }
                Change::Gone(file) => {
                    self.queue.retain(|r| r.file != file);
                    if self.asking.as_ref().is_some_and(|a| a.req.file == file) {
                        self.asking = None;
                        if !self.hint.is_empty() && self.submitted.is_some() {
                            self.hint.clear();
                        }
                        self.next_request(now);
                    }
                }
            }
        }
    }

    /// One frame. `size` is the window in logical pixels.
    pub fn tick(&mut self, app: &App, now: Usec, size: (f32, f32), scale: f32) -> Next {
        let dt = self.last.map_or(1.0 / 30.0, |l| (now.saturating_sub(l) as f32 / 1e6).clamp(0.0, 0.1));
        self.last = Some(now);
        self.poll_asks(now);

        if self.boot.stage == Stage::Finished && self.asking.is_none() {
            let at = *self.finished_at.get_or_insert(now);
            if now.saturating_sub(at) > self.cfg.screen.linger * 1000 {
                self.leaving = true;
            }
        }
        self.presence = if self.leaving { self.presence - dt / 0.35 } else { (self.presence + dt / 0.5).min(1.0) };
        if self.leaving && self.presence <= 0.0 {
            app.set_presence(0.0);
            return Next::Quit;
        }

        let (w, h) = size;
        let tiles = self.cfg.screen.tiles && w >= h * 1.3 && w >= 1100.0;
        let orbit = (h - 230.0).min(if tiles { w - 2.0 * 400.0 } else { w - 60.0 }).max(280.0);
        let center = (w / 2.0, h / 2.0 + 25.0);
        let fox_size = self.cfg.fox.size.min(orbit * 0.42);
        let g = Geometry {
            size: (orbit * scale).round(),
            inner: (fox_size / 2.0 * 1.05 + 22.0) * scale,
            outer: (orbit / 2.0 - 30.0) * scale,
            scale,
            dither: self.cfg.screen.dither.max(1),
        };

        self.scene.update(&self.boot, now, dt, &g, self.cfg.screen.bounce);
        let side = g.size as u32;
        if self.pix.as_ref().is_none_or(|p| p.width() != side) {
            self.pix = Pixmap::new(side.max(1), side.max(1));
        }
        if let Some(pix) = self.pix.as_mut() {
            self.scene.paint(pix, &self.pal, now, &g);
            let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(pix.data(), side, side);
            app.set_orbit(Image::from_rgba8_premultiplied(buf));
        }
        let labels: Vec<Label> = self
            .scene
            .labels(&self.pal, now, &g, center)
            .into_iter()
            .map(|l| Label {
                x: l.x,
                y: l.y,
                text: l.text.into(),
                color: l.color.slint(),
                opacity: l.opacity,
                side: l.side,
            })
            .collect();

        app.set_p(self.pal.slint());
        app.set_presence(self.presence.clamp(0.0, 1.0));
        app.set_orbit_size(orbit);
        app.set_center_x(center.0);
        app.set_center_y(center.1);
        app.set_fox_size(fox_size);
        let fox_px = if self.fox.has_frames() { 0 } else { ((fox_size * scale) as u32 / 4 * 4).max(64) };
        app.set_fox(self.fox.image(now as f32 / 1e6, &self.pal, fox_px));
        app.set_labels(ModelRc::new(VecModel::from(labels)));
        self.header(app, now);
        app.set_tiles(tiles);
        self.tiles(app, now);
        let c = self.boot.counts();
        app.set_done(c.done as i32);
        app.set_failed(c.failed as i32);
        app.set_busy(c.running as i32);
        app.set_waiting(c.waiting as i32);
        self.password(app, now, dt);

        if let Some(lines) = self.log_feed.as_ref().and_then(LogFeed::latest) {
            self.drawer_lines = lines;
        }
        let target = if self.drawer_open { 1.0 } else { 0.0 };
        self.drawer += (target - self.drawer) * (1.0 - (-dt * 12.0).exp());
        app.set_drawer(if (self.drawer - target).abs() < 0.005 { target } else { self.drawer });
        if self.drawer > 0.01 {
            let lines: Vec<SharedString> = self.drawer_lines.iter().map(SharedString::from).collect();
            app.set_drawer_lines(ModelRc::new(VecModel::from(lines)));
        }
        Next::Continue
    }

    fn header(&self, app: &App, now: Usec) {
        app.set_host(self.info.host.as_str().into());
        let shown = self.link_note.as_deref().or(self.note.as_deref()).unwrap_or(&self.info.os);
        app.set_os(shown.into());
        app.set_stage(text::stage(self.boot.stage).into());
        app.set_stage_color(match self.boot.stage {
            Stage::Finished => self.pal.green.slint(),
            Stage::SwitchingRoot | Stage::Connecting => self.pal.yellow.slint(),
            _ if self.boot.counts().failed > 0 => self.pal.red.slint(),
            _ => self.pal.accent.slint(),
        });
        let (clock, note) = match self.finished_at {
            Some(at) => (text::duration(at), "загрузилось за"),
            None => (text::duration(now), "с включения"),
        };
        app.set_clock(clock.into());
        app.set_clock_note(note.into());
    }

    fn tiles(&self, app: &App, now: Usec) {
        let mut running: Vec<_> = self.boot.jobs.values().filter(|j| j.state == JobState::Running).collect();
        running.sort_by_key(|j| self.boot.running_since(j));
        let rows: Vec<Row> = running
            .iter()
            .take(4)
            .map(|j| {
                let since = self.boot.running_since(j);
                let unit = self.boot.units.get(&j.unit);
                let timeout = unit.and_then(|u| u.facts.as_ref()).map_or(0, |f| f.timeout);
                let elapsed = now.saturating_sub(since);
                let label = unit.map_or(j.unit.as_str(), |u| u.label(&j.unit));
                Row {
                    name: label.into(),
                    value: text::duration(elapsed).into(),
                    detail: SharedString::new(),
                    color: self.pal.accent.slint(),
                    progress: if timeout > 0 { (elapsed as f32 / timeout as f32).min(1.0) } else { -1.0 },
                }
            })
            .collect();
        app.set_running(ModelRc::new(VecModel::from(rows)));

        let slowest = self.boot.slowest(4, self.cfg.screen.hide_runs_shorter_than * 1000);
        let max = slowest.first().map_or(1, |r| r.duration().max(1));
        let rows: Vec<Row> = slowest
            .iter()
            .map(|r| Row {
                name: {
                    let label = self.boot.units.get(&r.unit).map_or(r.unit.as_str(), |u| u.label(&r.unit));
                    label.strip_suffix(".service").unwrap_or(label).into()
                },
                value: text::duration(r.duration()).into(),
                detail: SharedString::new(),
                color: self.pal.accent.slint(),
                progress: r.duration() as f32 / max as f32,
            })
            .collect();
        app.set_slowest(ModelRc::new(VecModel::from(rows)));

        let shown: Vec<_> =
            self.boot.failures.iter().rev().filter(|f| f.kind != bento_boot_model::FailKind::Dependency).collect();
        let rows: Vec<Row> = shown
            .iter()
            .take(3)
            .map(|f| {
                let reason = if f.reason.is_empty() {
                    self.causes.get(&f.unit).cloned().unwrap_or_else(|| text::fallback(f.kind).to_owned())
                } else {
                    f.reason.clone()
                };
                Row {
                    name: f.unit.as_str().into(),
                    value: SharedString::new(),
                    detail: reason.into(),
                    color: self.pal.red.slint(),
                    progress: -1.0,
                }
            })
            .collect();
        let log: Vec<SharedString> = shown
            .first()
            .and_then(|f| self.unit_logs.get(&f.unit))
            .map(|l| l.iter().map(SharedString::from).collect())
            .unwrap_or_default();
        app.set_failure_log(ModelRc::new(VecModel::from(log)));
        app.set_more_failures(if shown.len() > 3 {
            format!(" +{}", shown.len() - 3).into()
        } else {
            SharedString::new()
        });
        app.set_failures(ModelRc::new(VecModel::from(rows)));
    }

    fn password(&mut self, app: &App, now: Usec, dt: f32) {
        let target = if self.asking.is_some() { 1.0 } else { 0.0 };
        self.ask_alpha += (target - self.ask_alpha) * (1.0 - (-dt * 10.0).exp());
        if (self.ask_alpha - target).abs() < 0.005 {
            self.ask_alpha = target;
        }
        app.set_asking(self.ask_alpha > 0.01);
        app.set_ask_opacity(self.ask_alpha);
        let shake = if now < self.shake_until {
            let left = (self.shake_until - now) as f32 / 1e6;
            (left * 70.0).sin() * 14.0 * (left / 0.35).min(1.0)
        } else {
            0.0
        };
        app.set_shake(shake);
        app.set_hint(self.hint.as_str().into());
        if let Some(a) = &self.asking {
            let n = a.prompt.dots().min(24);
            let dots: Vec<slint::Color> = (0..n)
                .map(|i| {
                    let t = if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 };
                    self.pal.accent2.mix(self.pal.accent, t).slint()
                })
                .collect();
            app.set_dots(ModelRc::new(VecModel::from(dots)));
            app.set_silent(a.req.silent);
            let q = if a.req.message.is_empty() { "Нужен пароль" } else { &a.req.message };
            app.set_question(q.into());
        }
    }
}
