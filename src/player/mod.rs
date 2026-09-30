//! Data the player layer exchanges, and the `Player` trait itself.
//!
//! Everything Spotify knows is read through one batched AppleScript call and
//! lands here. The types are deliberately dumb: no Spotify-specific quirks, no
//! formatting, no I/O. Rendering and formatting live in `cli/` and `tui/`.

pub mod applescript;
pub mod parse;

pub use applescript::AppleScriptPlayer;

/// What Spotify is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackState {
    Stopped,
    Playing,
    Paused,
}

impl PlaybackState {
    /// Parses the lowercase form AppleScript returns (`playing`, `paused`,
    /// `stopped`). The distributed notification uses a *capitalised* form, so
    /// that is handled separately in `player::notify`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "playing" => Some(Self::Playing),
            "paused" => Some(Self::Paused),
            "stopped" => Some(Self::Stopped),
            _ => None,
        }
    }

    /// Single letter used by the compact strip (TODO 3.4).
    #[allow(dead_code, reason = "used by the TUI, TODO 3.3")]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Playing => "▶",
            Self::Paused => "⏸",
            Self::Stopped => "⏹",
        }
    }
}

/// One track, as far as trak can see it.
///
/// Field-for-field with the batched read in `docs/APPLESCRIPT.md` §7. Values that
/// Spotify does not have are `None` rather than a sentinel: an advert really does
/// report an empty album, zero popularity and no artwork, and the Info tab has to
/// be able to tell "no artwork" from "artwork I failed to read".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    /// Milliseconds, as AppleScript reports it.
    pub duration_ms: u64,
    pub disc_number: u32,
    pub track_number: u32,
    /// 0–100. `None` when Spotify reported none (an advert reports 0).
    pub popularity: Option<u32>,
    pub play_count: Option<u32>,
    /// `None` when Spotify reported the literal string `missing value`.
    pub artwork_url: Option<String>,
    /// Full `spotify:track:…` URI. `None` for an advert, whose id is `spotify:ad:…`.
    pub uri: Option<String>,
}

impl TrackInfo {
    /// True when the "track" is really an advert or a promo, which has no track
    /// URI and no album (docs/APPLESCRIPT.md §3).
    pub fn is_ad(&self) -> bool {
        match &self.uri {
            Some(u) => !u.starts_with("spotify:track:"),
            None => true,
        }
    }

    /// Duration in whole seconds, rounded. Zero when unknown.
    pub fn duration_secs(&self) -> u64 {
        self.duration_ms / 1000
    }
}

/// How the player is set to repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    Off,
    Track,
    Context,
}

impl RepeatMode {
    /// Cycle order for the `r` key: off → all → one (TODO 2.4).
    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Context,
            Self::Context => Self::Track,
            Self::Track => Self::Off,
        }
    }

    #[allow(dead_code, reason = "used by the TUI, TODO 3.3")]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Off => "  ",
            Self::Context => "↻",
            Self::Track => "🔂",
        }
    }
}

/// A complete read of the player.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    pub playback: PlaybackState,
    /// Seconds, as AppleScript reports it. Float because the underlying value is
    /// an f32 widened to f64, so the digits are noisy (docs/APPLESCRIPT.md §2).
    pub position_secs: f64,
    /// The last volume trak *set*, not the raw read. Spotify quantises, so a read
    /// is often one lower than what was written (COMPAT rule 5, R2).
    pub volume: u8,
    /// The live shuffled state, which can differ from the toggle.
    pub shuffling_enabled: bool,
    pub repeating_enabled: bool,
    pub track: TrackInfo,
}

impl PlayerState {
    /// Progress as a fraction of the track, clamped to 0..=1. Zero when the
    /// duration is unknown (an advert) so callers never divide by zero.
    pub fn progress(&self) -> f64 {
        if self.track.duration_ms == 0 {
            return 0.0;
        }
        ((self.position_secs * 1000.0) / self.track.duration_ms as f64).clamp(0.0, 1.0)
    }

    /// Whether the position is advancing, which is what the TUI's local
    /// interpolation keys off (ARCHITECTURE "Progress bar is interpolated").
    #[allow(dead_code, reason = "used by the TUI, TODO 3.2")]
    pub fn is_playing(&self) -> bool {
        self.playback == PlaybackState::Playing
    }
}

/// Everything trak can ask Spotify to do.
///
/// Read calls are side-effect free and may be made as often as needed (COMPAT
/// rule 3). Write calls must only ever be made in direct response to a user key
/// or command, and never by a poll or a retry.
pub trait Player {
    fn state(&self) -> Result<PlayerState, PlayerError>;

    fn play(&self) -> Result<(), PlayerError>;
    fn pause(&self) -> Result<(), PlayerError>;
    fn toggle(&self) -> Result<(), PlayerError>;
    fn next(&self) -> Result<(), PlayerError>;
    fn previous(&self) -> Result<(), PlayerError>;

    /// Seek to an absolute position in seconds.
    fn seek(&mut self, secs: f64) -> Result<(), PlayerError>;

    /// Set Spotify's volume, 0–100. The caller reads back afterwards
    /// (COMPAT rule 5) and must allow ±1.
    fn set_volume(&mut self, volume: u8) -> Result<(), PlayerError>;

    /// Play a URI. Works for tracks, albums, playlists and artists, and works on
    /// the Free tier because it goes through AppleScript (SPEC §6).
    fn play_uri(&self, uri: &str) -> Result<(), PlayerError>;
}

/// Why a player call failed, in terms trak can act on.
#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    /// Spotify is not running. trak must never launch it (COMPAT rule 2).
    #[error("Spotify is not running")]
    NotRunning,

    /// The terminal is not allowed to control Spotify: AppleScript error -1743.
    /// Fixed in System Settings › Privacy & Security › Automation.
    #[error(
        "not allowed to control Spotify — allow your terminal in System Settings › Privacy & Security › Automation"
    )]
    PermissionDenied,

    /// osascript did not finish in time. 5 s; a normal call is ~430 ms and the
    /// worst observed was ~1.3 s (docs/APPLESCRIPT.md §4).
    #[error("timed out talking to Spotify")]
    Timeout,

    /// Anything osascript reported that is not one of the above.
    #[error("{0}")]
    Script(String),
}
