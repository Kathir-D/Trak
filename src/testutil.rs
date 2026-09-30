//! Test helpers shared by the unit tests.
//!
//! The fixtures under `tests/fixtures/` were captured from a real Spotify
//! (docs/APPLESCRIPT.md). Reading them at compile time means a fixture that
//! disappears is a build failure, not a silently skipped test.

use std::path::PathBuf;

pub fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// The raw stdout of one batched read, field-separated by U+001F.
pub fn fixture(name: &str) -> String {
    let path = fixtures_dir().join("applescript").join(name);
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading fixture {name}: {e}"));
    // compile_time-friendly: fail the build rather than a test if it vanishes
    raw
}
