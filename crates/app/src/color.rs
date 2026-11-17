//! Colours: parsing `#rrggbb` and mixing in OkLab (Björn Ottosson), so
//! gradients keep an even brightness instead of sagging through grey.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let h = s.strip_prefix('#')?;
        if h.len() != 6 || !h.is_ascii() {
            return None;
        }
        let c = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        Some(Self(c(0)?, c(2)?, c(4)?))
    }

    #[must_use]
    pub fn slint(self) -> slint::Color {
        slint::Color::from_rgb_u8(self.0, self.1, self.2)
    }

    #[must_use]
    pub fn skia(self, alpha: f32) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba8(self.0, self.1, self.2, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
    }

    /// `self` towards `other` by `t`, in OkLab.
    #[must_use]
    pub fn mix(self, other: Self, t: f32) -> Self {
        let a = to_oklab(self);
        let b = to_oklab(other);
        let t = t.clamp(0.0, 1.0);
        from_oklab([a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t])
    }
}

fn lin(c: u8) -> f32 {
    let x = f32::from(c) / 255.0;
    if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
}

fn srgb(x: f32) -> u8 {
    let x = x.clamp(0.0, 1.0);
    let y = if x <= 0.003_130_8 { 12.92 * x } else { 1.055 * x.powf(1.0 / 2.4) - 0.055 };
    (y * 255.0).round() as u8
}

/// OkLab lightness, 0 (black) to 1 (white).
#[must_use]
pub fn lightness(c: Rgb) -> f32 {
    to_oklab(c)[0]
}

fn to_oklab(c: Rgb) -> [f32; 3] {
    let (r, g, b) = (lin(c.0), lin(c.1), lin(c.2));
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn from_oklab([l_, a, b]: [f32; 3]) -> Rgb {
    let l = (l_ + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (l_ - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (l_ - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    Rgb(
        srgb(4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s),
        srgb(-1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s),
        srgb(-0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s),
    )
}

/// The palette, parsed once.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub base: Rgb,
    pub surface: Rgb,
    pub rings: Rgb,
    pub overlay: Rgb,
    pub subtext: Rgb,
    pub text: Rgb,
    pub accent: Rgb,
    pub accent2: Rgb,
    pub red: Rgb,
    pub green: Rgb,
    pub yellow: Rgb,
}

impl Palette {
    /// Unparsable colours fall back to the default palette's, one by one.
    #[must_use]
    pub fn from_config(c: &crate::config::Colors) -> Self {
        let d = crate::config::Colors::default();
        let pick = |s: &str, fallback: &str| Rgb::parse(s).or_else(|| Rgb::parse(fallback)).unwrap_or(Rgb(0, 0, 0));
        Self {
            base: pick(&c.base, &d.base),
            surface: pick(&c.surface, &d.surface),
            rings: pick(&c.rings, &d.rings),
            overlay: pick(&c.overlay, &d.overlay),
            subtext: pick(&c.subtext, &d.subtext),
            text: pick(&c.text, &d.text),
            accent: pick(&c.accent, &d.accent),
            accent2: pick(&c.accent2, &d.accent2),
            red: pick(&c.red, &d.red),
            green: pick(&c.green, &d.green),
            yellow: pick(&c.yellow, &d.yellow),
        }
    }

    #[must_use]
    pub fn slint(&self) -> crate::Palette {
        crate::Palette {
            base: self.base.slint(),
            surface: self.surface.slint(),
            rings: self.rings.slint(),
            overlay: self.overlay.slint(),
            subtext: self.subtext.slint(),
            text: self.text.slint(),
            accent: self.accent.slint(),
            accent2: self.accent2.slint(),
            red: self.red.slint(),
            green: self.green.slint(),
            yellow: self.yellow.slint(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing_ends_and_round_trip() {
        let a = Rgb::parse("#f38ba8").unwrap();
        let b = Rgb::parse("#cba6f7").unwrap();
        assert_eq!(a.mix(b, 0.0), a);
        assert_eq!(a.mix(b, 1.0), b);
        assert_eq!(Rgb::parse("#zzzzzz"), None);
        assert_eq!(Rgb::parse("#fff"), None);
    }
}
