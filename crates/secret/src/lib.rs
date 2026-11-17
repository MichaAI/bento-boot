//! A small systemd password agent.
//!
//! - [`buffer`]: the passphrase in one fixed, zeroed-on-drop array; the
//!   reply is a slice of it, so it is never copied.
//! - [`prompt`]: keystrokes to passphrase, sent at most once.
//! - [`ask`]: the strict parser for systemd's `ask.*` files.
//! - [`agent`]: watching `/run/systemd/ask-password` and answering.
//! - [`memory`]: no swap, no core dumps.
//!
//! The first three are pure and covered by randomized tests.

pub mod agent;
pub mod ask;
pub mod buffer;
pub mod memory;
pub mod prompt;

/// Room for a passphrase: 1023 bytes, more than any LUKS keyslot takes.
pub const PASSPHRASE_BYTES: usize = 1024;

pub type Passphrase = prompt::Prompt<PASSPHRASE_BYTES>;

#[cfg(test)]
mod random_tests;
