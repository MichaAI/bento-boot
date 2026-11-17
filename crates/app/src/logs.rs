//! The log drawer's feed: while the drawer is open, a background thread
//! reads the tail of the journal (or, before there is one, the kernel's
//! ring buffer) once in a while. Opening journal files can take a moment
//! on a big `/var/log/journal`, so it never runs on the drawing thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use bento_boot_systemd::{journal, kmsg::Kmsg};

const LINES: usize = 40;

pub struct LogFeed {
    open: Arc<AtomicBool>,
    rx: Receiver<Vec<String>>,
}

impl LogFeed {
    #[must_use]
    pub fn start() -> Option<Self> {
        let open = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let flag = open.clone();
        thread::Builder::new()
            .name("logs".into())
            .spawn(move || {
                let mut kmsg = Kmsg::open(LINES);
                loop {
                    let kernel: Vec<String> = kmsg.poll().iter().cloned().collect();
                    if flag.load(Ordering::Relaxed) {
                        let lines = journal::tail(LINES).filter(|l| !l.is_empty()).unwrap_or(kernel);
                        if tx.send(lines).is_err() {
                            return;
                        }
                    }
                    thread::sleep(Duration::from_millis(700));
                }
            })
            .ok()?;
        Some(Self { open, rx })
    }

    pub fn set_open(&self, open: bool) {
        self.open.store(open, Ordering::Relaxed);
    }

    /// The newest lines, if new ones came.
    pub fn latest(&self) -> Option<Vec<String>> {
        self.rx.try_iter().last()
    }
}
