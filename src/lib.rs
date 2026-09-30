//! trak — an interactive terminal UI for the Spotify desktop app on macOS.
//!
//! The library half of the crate. `main.rs` is a thin shell that parses argv and
//! calls into here; an example or an integration test can use any of it without
//! going through the binary, which is what makes the player layer testable.

pub mod accent;
pub mod art;
pub mod cli;
pub mod headless;
pub mod lyrics;
pub mod player;
pub mod sonar;
pub mod tui;

#[cfg(test)]
mod testutil;
