//! The thing in the middle: the fox's baked frames, dithered here into four
//! colours of the theme, or a dithered planet when there are none.

use std::path::Path;

use slint::{Image, Rgba8Pixel, SharedPixelBuffer};

use crate::color::{Palette, Rgb};

pub struct Fox {
    /// PNG bytes per frame; decoded one at a time, which keeps a few
    /// hundred kilobytes in memory instead of tens of megabytes.
    frames: Vec<Vec<u8>>,
    fps: f32,
    pixel: u32,
    palette: Option<[Rgb; 4]>,
    shown: Option<usize>,
    image: Image,
}

fn decode(png_bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(png_bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => {
            buf[..info.buffer_size()].chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect()
        }
        png::ColorType::GrayscaleAlpha => {
            buf[..info.buffer_size()].chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect()
        }
        png::ColorType::Grayscale => buf[..info.buffer_size()].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some((rgba, w, h))
}

/// Ordered dithering into four colours: the frame is cut into `cell`
/// squares, each takes the colour of its centre pixel, its OkLab lightness
/// plus the Bayer threshold picks one of the colours, and coverage is
/// dithered the same way, so there are no half-transparent edges.
fn dither(rgba: &[u8], w: u32, h: u32, cell: u32, colors: &[Rgb; 4]) -> SharedPixelBuffer<Rgba8Pixel> {
    let cell = cell.max(1);
    let mut out = SharedPixelBuffer::<Rgba8Pixel>::new(w, h);
    let px = out.make_mut_slice();
    for gy in 0..h.div_ceil(cell) {
        for gx in 0..w.div_ceil(cell) {
            let (cx, cy) = ((gx * cell + cell / 2).min(w - 1), (gy * cell + cell / 2).min(h - 1));
            let i = ((cy * w + cx) * 4) as usize;
            let (r, g, b, a) = (rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]);
            let t = (BAYER[(gy & 3) as usize][(gx & 3) as usize] + 0.5) / 16.0;
            if f32::from(a) / 255.0 < t {
                continue;
            }
            let l = crate::color::lightness(Rgb(r, g, b));
            let idx = ((l * 3.0 + t).floor() as usize).min(3);
            let c = colors[idx];
            for y in gy * cell..((gy + 1) * cell).min(h) {
                for x in gx * cell..((gx + 1) * cell).min(w) {
                    px[(y * w + x) as usize] = Rgba8Pixel { r: c.0, g: c.1, b: c.2, a: 255 };
                }
            }
        }
    }
    out
}

/// The four fox colours: from the config, or picked from the theme
/// (Catppuccin Mocha gives the lock screen's surface, mauve, pink and
/// rosewater-ish steps).
#[must_use]
pub fn palette(cfg: &[String], pal: &Palette) -> [Rgb; 4] {
    let parsed: Vec<Rgb> = cfg.iter().filter_map(|s| Rgb::parse(s)).collect();
    if let [a, b, c, d] = parsed[..] {
        return [a, b, c, d];
    }
    [pal.rings.mix(pal.overlay, 0.3), pal.accent, pal.accent.mix(pal.text, 0.55), pal.text.mix(pal.accent2, 0.15)]
}

impl Fox {
    /// Loads `frame-0.png`, `frame-1.png`, … from `dir`.
    #[must_use]
    pub fn load(dir: Option<&Path>, fps: f32, pixel: u32, palette: Option<[Rgb; 4]>) -> Self {
        let mut frames = Vec::new();
        if let Some(dir) = dir {
            while let Ok(bytes) = std::fs::read(dir.join(format!("frame-{}.png", frames.len()))) {
                frames.push(bytes);
            }
        }
        Self { frames, fps: fps.max(1.0), pixel, palette, shown: None, image: Image::default() }
    }

    #[must_use]
    pub fn has_frames(&self) -> bool {
        !self.frames.is_empty()
    }

    /// The image for time `t` (seconds).
    pub fn image(&mut self, t: f32, pal: &Palette, side: u32) -> Image {
        if self.frames.is_empty() {
            return planet(t, pal, side);
        }
        let i = (t * self.fps) as usize % self.frames.len();
        if self.shown != Some(i)
            && let Some((rgba, w, h)) = decode(&self.frames[i])
        {
            let colors = self.palette.unwrap_or_else(|| palette(&[], pal));
            self.image = Image::from_rgba8(dither(&rgba, w, h, self.pixel, &colors));
            self.shown = Some(i);
        }
        self.image.clone()
    }
}

const BAYER: [[f32; 4]; 4] =
    [[0.0, 8.0, 2.0, 10.0], [12.0, 4.0, 14.0, 6.0], [3.0, 11.0, 1.0, 9.0], [15.0, 7.0, 13.0, 5.0]];

/// A small turning planet with bands, dithered into four colours of the
/// palette, for when no fox was baked.
fn planet(t: f32, pal: &Palette, side: u32) -> Image {
    let cell = 4u32;
    let n = side / cell;
    let colors = [pal.rings, pal.accent, pal.accent.mix(pal.accent2, 0.5), pal.text.mix(pal.accent2, 0.3)];
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(side, side);
    let px = buf.make_mut_slice();
    let r = n as f32 * 0.36;
    let c = n as f32 / 2.0;
    for gy in 0..n {
        for gx in 0..n {
            let (x, y) = ((gx as f32 + 0.5 - c) / r, (gy as f32 + 0.5 - c) / r);
            let d2 = x * x + y * y;
            let threshold = (BAYER[(gy & 3) as usize][(gx & 3) as usize] + 0.5) / 16.0;
            let color: Option<Rgb> = if d2 <= 1.0 {
                let z = (1.0 - d2).sqrt();
                // Light from the upper left, bands that drift with time.
                let light = (0.55 * (-x * 0.5 - y * 0.6 + z * 0.65) + 0.45).clamp(0.0, 1.0);
                let lon = x.atan2(z) + t * 0.6;
                let band = 0.5 + 0.5 * (y * 5.0 + (lon * 2.0).sin() * 0.6).sin();
                let l = (light * 0.85 + band * 0.25).clamp(0.0, 0.999);
                let idx = ((l * 3.0 + threshold).floor() as usize).min(3);
                Some(colors[idx])
            } else {
                // A thin ring around it.
                let ry = y * 3.2;
                let e = (x * x + ry * ry).sqrt();
                let behind = y < 0.0 && d2 < 1.0;
                ((1.25..1.42).contains(&e) && !behind && threshold < 0.8).then_some(pal.subtext)
            };
            if let Some(col) = color {
                for yy in 0..cell {
                    for xx in 0..cell {
                        let i = ((gy * cell + yy) * side + gx * cell + xx) as usize;
                        px[i] = Rgba8Pixel { r: col.0, g: col.1, b: col.2, a: 255 };
                    }
                }
            }
        }
    }
    Image::from_rgba8(buf)
}
