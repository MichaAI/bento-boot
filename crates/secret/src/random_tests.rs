//! Randomized checks of the invariants the passphrase path relies on,
//! against a plain reference model. Deterministic seeds, so a failure
//! reproduces.

use crate::ask;
use crate::buffer::SecretBuf;
use crate::prompt::{Key, Outcome, Prompt};

/// xorshift64*, enough to shake out edge cases without a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// Mostly ASCII, some Cyrillic, emoji and control characters.
    fn char(&mut self) -> char {
        let pick = match self.below(10) {
            0..=4 => 0x20 + self.below(0x5f),
            5 | 6 => 0x430 + self.below(0x20),
            7 => 0x1f300 + self.below(0x300),
            8 => self.below(0x20),
            _ => self.below(0x11_0000),
        };
        char::from_u32(pick as u32).unwrap_or('?')
    }
}

#[test]
fn buffer_matches_a_reference_model() {
    for seed in 1..=500u64 {
        let mut rng = Rng(seed);
        let mut buf = SecretBuf::<17>::new();
        let mut model = String::new();
        for _ in 0..200 {
            match rng.below(4) {
                0 | 1 => {
                    let c = rng.char();
                    let fits = model.len() + c.len_utf8() <= SecretBuf::<17>::capacity();
                    assert_eq!(buf.push(c).is_ok(), fits);
                    if fits {
                        model.push(c);
                    }
                }
                2 => assert_eq!(buf.pop(), model.pop().is_some()),
                _ => {
                    if rng.below(8) == 0 {
                        buf.clear();
                        model.clear();
                    }
                }
            }
            assert_eq!(buf.passphrase(), model.as_bytes());
            assert_eq!(buf.chars(), model.chars().count());
            assert_eq!(buf.reply()[0], b'+');
            assert_eq!(&buf.reply()[1..], model.as_bytes());
            assert!(buf.tail_is_zero(), "seed {seed}: bytes left behind after the passphrase");
        }
    }
}

#[test]
fn prompt_sends_once_and_never_stores_control_characters() {
    for seed in 1..=500u64 {
        let mut rng = Rng(seed);
        let mut p = Prompt::<33>::new();
        let mut sent: Option<Vec<u8>> = None;
        for _ in 0..100 {
            let key = match rng.below(12) {
                0 => Key::Backspace,
                1 => Key::ClearLine,
                2 => Key::Enter,
                3 if rng.below(4) == 0 => Key::Escape,
                _ => Key::Char(rng.char()),
            };
            let out = p.key(key);
            match &sent {
                Some(s) => {
                    assert_eq!(out, Outcome::Ignored);
                    assert_eq!(p.reply(), Some(&s[..]));
                }
                None => {
                    if matches!(out, Outcome::Submit | Outcome::Cancel) {
                        let r = p.reply().expect("reply after submit").to_vec();
                        if out == Outcome::Cancel {
                            assert_eq!(r, b"+");
                        }
                        sent = Some(r);
                    } else {
                        assert_eq!(p.reply(), None);
                    }
                }
            }
            let s = std::str::from_utf8(p.buffer().passphrase()).expect("valid UTF-8");
            assert!(s.chars().all(|c| !c.is_control()), "seed {seed}");
        }
    }
}

#[test]
fn parser_never_panics_and_only_accepts_the_ask_dir() {
    let pieces: [&[u8]; 12] = [
        b"[Ask]\n",
        b"[Other]\n",
        b"Socket=/run/systemd/ask-password/sck.",
        b"Socket=/tmp/x\n",
        b"../",
        b"PID=",
        b"99999999999999999999999",
        b"NotAfter=12\n",
        b"Echo=maybe\n",
        b"Message=\xff\xfe hi\n",
        b"=\n",
        b"\n",
    ];
    for seed in 1..=20_000u64 {
        let mut rng = Rng(seed);
        let mut input = Vec::new();
        for _ in 0..rng.below(10) {
            if rng.below(3) == 0 {
                input.push(rng.next() as u8);
            } else {
                input.extend_from_slice(pieces[rng.below(pieces.len() as u64) as usize]);
            }
        }
        if let Ok(a) = ask::parse(&input) {
            assert!(ask::socket_path_ok(a.socket));
            assert!(a.socket.starts_with(ask::ASK_DIR));
            assert!(!a.socket[ask::ASK_DIR.len()..].contains(&b'/'));
        }
    }
}
