//! Public visual state for the greeter. No request text or password data.

use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::time::{SystemTime, UNIX_EPOCH};

const STATE: &str = "/run/bento-boot-handoff.json";
const TEMP: &str = "/run/bento-boot-handoff.json.tmp";

pub fn clear() {
    let _ = std::fs::remove_file(STATE);
}

pub fn save(frame: usize, x: f32, y: f32, size: f32) -> io::Result<()> {
    let saved = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    let _ = std::fs::remove_file(TEMP);
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o644).open(TEMP)?;
    writeln!(file, "{{\"version\":1,\"savedAt\":{saved},\"frame\":{frame},\"x\":{x},\"y\":{y},\"size\":{size}}}")?;
    std::fs::rename(TEMP, STATE)
}
