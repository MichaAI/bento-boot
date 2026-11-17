//! `bento-boot preview`: plays the scripted boot with a simulated clock
//! and writes frames as PNG (and optionally one animated PNG), using
//! Slint's software renderer without any display.

use std::cell::Cell;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use bento_boot_secret::agent::Change;
use slint::platform::software_renderer::{MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, PhysicalSize};

use crate::App;
use crate::config::Config;
use crate::controller::{Controller, SystemInfo};
use crate::scripted::{self, ScriptedAsker, Step};

struct PreviewPlatform {
    window: Rc<MinimalSoftwareWindow>,
    now: Rc<Cell<Duration>>,
}

impl Platform for PreviewPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.now.get()
    }
}

pub struct Options {
    pub out: PathBuf,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    /// Seconds of the script to save stills at.
    pub stills: Vec<f64>,
    /// Frames per second of an animated PNG; 0 for none.
    pub apng_fps: u32,
    pub config: Config,
}

fn sample_info() -> SystemInfo {
    SystemInfo { host: "lisa".into(), os: "NixOS 26.05 (Yarara) · 6.18.4".into() }
}

fn save_png(path: &Path, w: u32, h: u32, rgb: &[u8]) -> std::io::Result<()> {
    let mut enc = png::Encoder::new(BufWriter::new(File::create(path)?), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().map_err(std::io::Error::other)?;
    wr.write_image_data(rgb).map_err(std::io::Error::other)?;
    Ok(())
}

pub fn run(opts: Options) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(&opts.out)?;
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let now = Rc::new(Cell::new(Duration::ZERO));
    slint::platform::set_platform(Box::new(PreviewPlatform { window: window.clone(), now: now.clone() }))
        .map_err(|e| format!("{e:?}"))?;

    let app = App::new()?;
    window.set_size(PhysicalSize::new(opts.width, opts.height));
    window.dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: opts.scale });
    app.show()?;

    let asker = ScriptedAsker::default();
    let mut ctl = Controller::new(opts.config, Box::new(asker.clone()), sample_info);
    ctl.set_drawer_lines(scripted::drawer_lines());
    let steps = scripted::boot();
    let end = steps.last().map_or(0, |(t, _)| *t) + 2_000_000;

    let (w, h) = (opts.width, opts.height);
    let logical = (w as f32 / opts.scale, h as f32 / opts.scale);
    let mut buf = vec![PremultipliedRgbaColor::default(); (w * h) as usize];
    let mut rgb = vec![0u8; (w * h * 3) as usize];

    let mut stills = opts.stills.clone();
    stills.sort_by(f64::total_cmp);
    let mut stills = stills.into_iter().map(|s| (s * 1e6) as u64).peekable();

    let frame_us = 1_000_000 / 30;
    let apng_every = if opts.apng_fps > 0 { 1_000_000 / u64::from(opts.apng_fps) } else { u64::MAX };
    let apng_frames = if opts.apng_fps > 0 { (end / apng_every) as u32 } else { 0 };
    let mut apng = if apng_frames > 0 {
        let file = BufWriter::new(File::create(opts.out.join("boot.png"))?);
        let mut enc = png::Encoder::new(file, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        enc.set_animated(apng_frames, 0)?;
        enc.set_frame_delay(1, u16::try_from(opts.apng_fps).unwrap_or(15))?;
        Some(enc.write_header()?)
    } else {
        None
    };
    let mut apng_written = 0u32;
    let mut next_apng = 0u64;

    let mut i = 0;
    let mut t = 0u64;
    while t <= end {
        while i < steps.len() && steps[i].0 <= t {
            match &steps[i].1 {
                Step::Msg(m) => ctl.feed(m.clone(), t),
                Step::Ask(r) => asker.push(Change::Asked(r.clone())),
                Step::AskGone(p) => asker.push(Change::Gone(p.clone())),
                Step::Key(k) => ctl.key(k, false, t),
            }
            i += 1;
        }
        now.set(Duration::from_micros(t));
        ctl.tick(&app, t, logical, opts.scale);
        slint::platform::update_timers_and_animations();

        let want_still = stills.peek().is_some_and(|&s| t >= s);
        let want_apng = apng.is_some() && t >= next_apng && apng_written < apng_frames;
        if want_still || want_apng {
            window.request_redraw();
            window.draw_if_needed(|r| {
                r.render(&mut buf, w as usize);
            });
            for (p, out) in buf.iter().zip(rgb.chunks_exact_mut(3)) {
                out.copy_from_slice(&[p.red, p.green, p.blue]);
            }
            if want_still {
                let s = stills.next().unwrap_or(0);
                save_png(&opts.out.join(format!("t{:05.1}.png", s as f64 / 1e6)), w, h, &rgb)?;
            }
            if want_apng && let Some(wr) = apng.as_mut() {
                wr.write_image_data(&rgb)?;
                apng_written += 1;
                next_apng += apng_every;
            }
        }
        t += frame_us;
    }
    if let Some(wr) = apng
        && apng_written == apng_frames
    {
        wr.finish()?;
    }
    let answers = asker.answers.borrow();
    eprintln!("preview: {} answers sent, last one {} bytes", answers.len(), answers.last().map_or(0, Vec::len));
    Ok(())
}
