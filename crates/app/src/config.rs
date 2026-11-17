//! `/etc/bento-boot.toml`: colours, the fox and a few knobs. Every key is
//! optional; the defaults are Catppuccin Mocha, as on the lock screen.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Colors {
    pub base: String,
    pub surface: String,
    pub rings: String,
    pub overlay: String,
    pub subtext: String,
    pub text: String,
    /// Running units, the critical path's far end, the password field.
    pub accent: String,
    /// The critical path's near end and the start of the dot gradient.
    pub accent2: String,
    pub red: String,
    pub green: String,
    pub yellow: String,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            base: "#11111b".into(),
            surface: "#1e1e2e".into(),
            rings: "#313244".into(),
            overlay: "#6c7086".into(),
            subtext: "#a6adc8".into(),
            text: "#cdd6f4".into(),
            accent: "#cba6f7".into(),
            accent2: "#f38ba8".into(),
            red: "#f38ba8".into(),
            green: "#a6e3a1".into(),
            yellow: "#f9e2af".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Fox {
    /// Directory with `frame-0.png` … `frame-N.png`, baked by the Nix
    /// build. Without it the centre shows a dithered planet.
    pub frames: Option<PathBuf>,
    pub fps: f32,
    /// Side of the fox on screen, logical pixels.
    pub size: f32,
    /// Cell of the fox's dithering, in frame pixels.
    pub pixel: u32,
    /// Four colours, dark to light, the fox is dithered into; empty takes
    /// them from the theme colours.
    pub palette: Vec<String>,
}

impl Default for Fox {
    fn default() -> Self {
        Self { frames: None, fps: 30.0, size: 240.0, pixel: 4, palette: Vec::new() }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Screen {
    /// Logical-to-physical scale; 0 picks one from the screen height.
    pub scale: f32,
    /// Cell of the dithering on halos, physical pixels.
    pub dither: u32,
    /// Side tiles (running, slowest, failures).
    pub tiles: bool,
    /// Runs shorter than this stay off the "slowest" tile, milliseconds.
    pub hide_runs_shorter_than: u64,
    /// After the boot is finished, stay this long before leaving, ms.
    pub linger: u64,
    /// How bouncy the dots are: 0 is a plain glide, 1 wobbles a lot.
    pub bounce: f32,
}

impl Default for Screen {
    fn default() -> Self {
        Self { scale: 0.0, dither: 3, tiles: true, hide_runs_shorter_than: 300, linger: 700, bounce: 0.45 }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub colors: Colors,
    pub fox: Fox,
    pub screen: Screen,
}

pub const DEFAULT_PATH: &str = "/etc/bento-boot.toml";

impl Config {
    /// Reads the file; a missing file gives the defaults, a broken one the
    /// defaults plus the reason (the boot screen must come up regardless).
    #[must_use]
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match std::fs::read_to_string(path) {
            Ok(s) => match toml::from_str(&s) {
                Ok(c) => (c, None),
                Err(e) => (Self::default(), Some(format!("{}: {e}", path.display()))),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (Self::default(), Some(format!("{}: {e}", path.display()))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_files_keep_defaults() {
        let c: Config = toml::from_str("[colors]\naccent = \"#89b4fa\"\n[screen]\ntiles = false\n").unwrap();
        assert_eq!(c.colors.accent, "#89b4fa");
        assert_eq!(c.colors.base, "#11111b");
        assert!(!c.screen.tiles);
        assert!(toml::from_str::<Config>("[colors]\nacent = \"#000\"\n").is_err());
    }
}
