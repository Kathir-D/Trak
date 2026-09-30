//! Version A: the Spotify Web API.
//!
//! trak is a control surface for the desktop app, not a client for the cloud, so
//! the Web API is strictly additive: search, playlists, queue and library. All
//! *playback* still goes through AppleScript, because that works on a Free
//! account and the Web API's own playback endpoints are gone in dev mode
//! (`docs/WEB-API.md` §0).
//!
//! The three modules split along the lines of what has to be testable without a
//! network:
//!
//! - [`auth`] — PKCE, behind an injectable token endpoint and browser, with a
//!   real loopback listener because that is the only part that cannot be faked.
//! - [`token`] — the `0600` file, not the Keychain. `docs/KEYCHAIN.md` records
//!   why: a Keychain item is bound to the exact binary that wrote it, so every
//!   `brew upgrade` would strand the user at an authorization prompt nobody can
//!   click.
//! - [`api`] — the `Library` trait, the real client, and a fake, so the tabs
//!   written against the trait never need Spotify to be running.

pub mod api;
pub mod auth;
pub mod token;
