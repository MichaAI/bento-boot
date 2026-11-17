//! Keystrokes to a passphrase, as a small state machine.
//!
//! Once the passphrase is sent (or the prompt cancelled), every further key
//! is ignored: nothing can change what was sent, and nothing can send it
//! twice. Control characters never end up in the passphrase.

use crate::buffer::SecretBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Backspace,
    /// Ctrl+U: forget everything typed.
    ClearLine,
    Enter,
    Escape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing changed.
    Ignored,
    /// The passphrase changed; redraw the dots.
    Changed,
    /// The key could not be taken (control character, or no room); the
    /// screen may shake.
    Rejected,
    /// Send [`Prompt::reply`].
    Submit,
    /// Tell the asker no passphrase is coming.
    Cancel,
}

pub struct Prompt<const N: usize> {
    buf: SecretBuf<N>,
    done: bool,
}

impl<const N: usize> Default for Prompt<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Prompt<N> {
    #[must_use]
    pub const fn new() -> Self {
        Self { buf: SecretBuf::new(), done: false }
    }

    pub fn key(&mut self, key: Key) -> Outcome {
        if self.done {
            return Outcome::Ignored;
        }
        match key {
            Key::Char(c) if c.is_control() => Outcome::Rejected,
            Key::Char(c) => match self.buf.push(c) {
                Ok(()) => Outcome::Changed,
                Err(_) => Outcome::Rejected,
            },
            Key::Backspace => {
                if self.buf.pop() {
                    Outcome::Changed
                } else {
                    Outcome::Ignored
                }
            }
            Key::ClearLine => {
                if self.buf.is_empty() {
                    Outcome::Ignored
                } else {
                    self.buf.clear();
                    Outcome::Changed
                }
            }
            Key::Enter => {
                self.done = true;
                Outcome::Submit
            }
            Key::Escape => {
                self.done = true;
                self.buf.clear();
                Outcome::Cancel
            }
        }
    }

    /// Dots to show.
    #[must_use]
    pub fn dots(&self) -> usize {
        self.buf.chars()
    }

    #[must_use]
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// The datagram to send once [`Outcome::Submit`] came back.
    #[must_use]
    pub fn reply(&self) -> Option<&[u8]> {
        self.done.then(|| self.buf.reply())
    }

    /// Forget the passphrase, after it was sent.
    pub fn wipe(&mut self) {
        self.buf.clear();
    }

    #[must_use]
    pub fn buffer(&self) -> &SecretBuf<N> {
        &self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_sending() {
        let mut p = Prompt::<32>::new();
        for c in "hunter2".chars() {
            assert_eq!(p.key(Key::Char(c)), Outcome::Changed);
        }
        assert_eq!(p.key(Key::Char('\u{7}')), Outcome::Rejected);
        assert_eq!(p.key(Key::Backspace), Outcome::Changed);
        assert_eq!(p.dots(), 6);
        assert_eq!(p.reply(), None);
        assert_eq!(p.key(Key::Enter), Outcome::Submit);
        assert_eq!(p.reply(), Some(&b"+hunter"[..]));
        assert_eq!(p.key(Key::Char('x')), Outcome::Ignored);
        assert_eq!(p.key(Key::Enter), Outcome::Ignored);
        p.wipe();
        assert_eq!(p.reply(), Some(&b"+"[..]));
    }

    #[test]
    fn escape_cancels_and_forgets() {
        let mut p = Prompt::<8>::new();
        p.key(Key::Char('a'));
        assert_eq!(p.key(Key::Escape), Outcome::Cancel);
        assert_eq!(p.dots(), 0);
    }
}
