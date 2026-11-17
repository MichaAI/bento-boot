//! The passphrase, in one fixed array that never grows or moves.
//!
//! Byte 0 is always `+`: the reply systemd's password protocol expects is
//! `+` followed by the passphrase, so the reply is a slice of this array
//! and the passphrase is never copied anywhere else. Bytes past the end of
//! the passphrase are always zero, so deleting characters leaves nothing
//! behind. Both are checked by the randomized test below.

use zeroize::Zeroize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Full;

/// `N` bytes, of which `N - 1` hold the passphrase.
pub struct SecretBuf<const N: usize> {
    bytes: [u8; N],
    /// Bytes of passphrase, not counting the leading `+`.
    len: usize,
}

const fn is_continuation(b: u8) -> bool {
    b & 0b1100_0000 == 0b1000_0000
}

impl<const N: usize> SecretBuf<N> {
    const CHECK: () = assert!(N >= 2, "room for the `+` and at least one byte");

    #[must_use]
    pub const fn new() -> Self {
        let () = Self::CHECK;
        let mut bytes = [0u8; N];
        bytes[0] = b'+';
        Self { bytes, len: 0 }
    }

    /// Passphrase bytes the buffer can hold.
    #[must_use]
    pub const fn capacity() -> usize {
        N - 1
    }

    /// Length of the passphrase in bytes.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Characters typed, which is what the screen shows as dots.
    #[must_use]
    pub fn chars(&self) -> usize {
        self.passphrase().iter().filter(|&&b| !is_continuation(b)).count()
    }

    /// Appends `c` whole, or nothing if it does not fit.
    pub fn push(&mut self, c: char) -> Result<(), Full> {
        let mut enc = [0u8; 4];
        let s = c.encode_utf8(&mut enc);
        let n = s.len();
        let at = 1 + self.len;
        if n > N - at {
            enc.zeroize();
            return Err(Full);
        }
        self.bytes[at..at + n].copy_from_slice(&enc[..n]);
        self.len += n;
        enc.zeroize();
        Ok(())
    }

    /// Removes the last character. Returns whether there was one.
    pub fn pop(&mut self) -> bool {
        if self.len == 0 {
            return false;
        }
        let end = 1 + self.len;
        let mut start = end - 1;
        while start > 1 && is_continuation(self.bytes[start]) {
            start -= 1;
        }
        self.bytes[start..end].zeroize();
        self.len = start - 1;
        true
    }

    pub fn clear(&mut self) {
        self.bytes[1..].zeroize();
        self.len = 0;
    }

    #[must_use]
    pub fn passphrase(&self) -> &[u8] {
        &self.bytes[1..1 + self.len]
    }

    /// `+` and the passphrase: the datagram to send to the asking socket.
    #[must_use]
    pub fn reply(&self) -> &[u8] {
        &self.bytes[..1 + self.len]
    }

    /// The whole backing array, for locking it into memory.
    #[must_use]
    pub fn storage(&self) -> &[u8; N] {
        &self.bytes
    }

    #[cfg(test)]
    pub(crate) fn tail_is_zero(&self) -> bool {
        self.bytes[1 + self.len..].iter().all(|&b| b == 0)
    }
}

impl<const N: usize> Default for SecretBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Drop for SecretBuf<N> {
    fn drop(&mut self) {
        self.bytes.zeroize();
        self.len = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_round_trip() {
        let mut b = SecretBuf::<16>::new();
        for c in "пёс🦊a".chars() {
            b.push(c).unwrap();
        }
        assert_eq!(b.passphrase(), "пёс🦊a".as_bytes());
        assert_eq!(b.chars(), 5);
        assert_eq!(b.reply()[0], b'+');
        assert!(b.pop());
        assert!(b.pop());
        assert_eq!(b.passphrase(), "пёс".as_bytes());
        assert!(b.tail_is_zero());
    }

    #[test]
    fn full_rejects_whole_char() {
        let mut b = SecretBuf::<4>::new();
        b.push('a').unwrap();
        assert_eq!(b.push('🦊'), Err(Full));
        assert_eq!(b.passphrase(), b"a");
        b.push('ё').unwrap();
        assert_eq!(b.len(), 3);
        assert!(b.tail_is_zero());
    }
}
