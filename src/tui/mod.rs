//! The terminal UI: state, rendering, and the event loop.
//!
//! Split so each piece is testable on its own. `app` is pure, `render` only
//! draws, and `loop_` is the only thing that talks to the terminal or a player.

pub mod app;
pub mod bidi;
pub mod colour;
pub mod loop_;
pub mod render;
pub mod settings;
pub mod setup;
pub mod theme;
pub mod web_tabs;

pub use loop_::{config_screen, run};
