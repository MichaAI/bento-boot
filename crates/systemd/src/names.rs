//! Object paths to unit names and job ids.
//!
//! Units live at `/org/freedesktop/systemd1/unit/<name>`, with every byte
//! outside `[A-Za-z0-9]` written as `_xx` in hex (sd-bus label escaping);
//! jobs at `/org/freedesktop/systemd1/job/<id>`.

const UNIT_PREFIX: &str = "/org/freedesktop/systemd1/unit/";
const JOB_PREFIX: &str = "/org/freedesktop/systemd1/job/";

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[must_use]
pub fn unit_from_path(path: &str) -> Option<String> {
    let label = path.strip_prefix(UNIT_PREFIX)?.as_bytes();
    let mut out = Vec::with_capacity(label.len());
    let mut i = 0;
    while i < label.len() {
        if label[i] == b'_' {
            let hi = hex(*label.get(i + 1)?)?;
            let lo = hex(*label.get(i + 2)?)?;
            out.push(hi << 4 | lo);
            i += 3;
        } else {
            out.push(label[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[must_use]
pub fn job_id_from_path(path: &str) -> Option<u32> {
    path.strip_prefix(JOB_PREFIX)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_unit_paths() {
        assert_eq!(
            unit_from_path("/org/freedesktop/systemd1/unit/systemd_2djournald_2eservice").as_deref(),
            Some("systemd-journald.service")
        );
        assert_eq!(unit_from_path("/org/freedesktop/systemd1/unit/_2d_2emount").as_deref(), Some("-.mount"));
        assert_eq!(unit_from_path("/org/freedesktop/systemd1/unit/bad_2"), None);
        assert_eq!(job_id_from_path("/org/freedesktop/systemd1/job/417"), Some(417));
        assert_eq!(job_id_from_path("/org/freedesktop/systemd1/unit/x"), None);
    }
}
