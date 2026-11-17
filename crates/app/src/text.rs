//! Words on the screen.

use bento_boot_model::{FailKind, Stage, Usec};
use bento_boot_systemd::Cause;

/// `0.4 с`, `12 с`, `2 мин 05 с`.
#[must_use]
pub fn duration(us: Usec) -> String {
    let s = us as f64 / 1e6;
    if s < 10.0 {
        format!("{s:.1} с")
    } else if s < 60.0 {
        format!("{s:.0} с")
    } else {
        let s = s as u64;
        format!("{} мин {:02} с", s / 60, s % 60)
    }
}

#[must_use]
pub fn stage(s: Stage) -> &'static str {
    match s {
        Stage::Connecting => "ищу systemd",
        Stage::Initrd => "initrd",
        Stage::SwitchingRoot => "переезд в корень",
        Stage::Main => "система",
        Stage::Finished => "готово",
    }
}

fn signal(n: i32) -> String {
    let name = match n {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        14 => "SIGALRM",
        15 => "SIGTERM",
        _ => return format!("сигналом {n}"),
    };
    name.to_owned()
}

/// Why a unit failed, in a few words.
#[must_use]
pub fn cause(c: &Cause) -> String {
    const CLD_EXITED: i32 = 1;
    let base = match c.result.as_str() {
        "exit-code" if c.code == CLD_EXITED => format!("вышел с кодом {}", c.status),
        "exit-code" => "вышел с ошибкой".to_owned(),
        "signal" => format!("убит {}", signal(c.status)),
        "core-dump" => format!("упал с дампом, {}", signal(c.status)),
        "timeout" => "не успел вовремя".to_owned(),
        "start-limit-hit" => "слишком часто перезапускался".to_owned(),
        "oom-kill" => "убит: кончилась память".to_owned(),
        "resources" => "не хватило ресурсов".to_owned(),
        "watchdog" => "перестал отвечать (watchdog)".to_owned(),
        "exec-condition" => "условие запуска не выполнено".to_owned(),
        "protocol" => "нарушил протокол запуска".to_owned(),
        "" | "success" => "упал".to_owned(),
        other => other.to_owned(),
    };
    let status = c.status_text.trim();
    if status.is_empty() { base } else { format!("{base}: {status}") }
}

#[must_use]
pub fn fallback(kind: FailKind) -> &'static str {
    match kind {
        FailKind::Broke => "упал",
        FailKind::Timeout => "не успел вовремя",
        FailKind::Dependency => "из-за зависимости",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words() {
        assert_eq!(duration(1_250_000), "1.2 с");
        assert_eq!(duration(75_000_000), "1 мин 15 с");
        let c = Cause { result: "exit-code".into(), code: 1, status: 2, status_text: "no db".into() };
        assert_eq!(cause(&c), "вышел с кодом 2: no db");
        let c = Cause { result: "signal".into(), code: 2, status: 11, status_text: String::new() };
        assert_eq!(cause(&c), "убит SIGSEGV");
    }
}
