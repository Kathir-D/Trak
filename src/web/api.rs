//! The Spotify Web API surface: the `Library` seam and the two implementations
//! behind it (TODO 7.5, SPEC §6).
//!
//! **`docs/WEB-API.md` is the authority here**, and it is not a summary of the
//! API — it is a record of what developer mode *removed* from it. Five of its
//! facts shape this module and every one of them would be a bug if forgotten:
//!
//! - **Every multi-item "get several" endpoint is gone.** `GET /tracks`,
//!   `GET /albums`, `GET /artists` and the rest were removed in February 2026.
//!   There is no batch helper anywhere in this file, because there is nothing on
//!   the server to point one at: one id, one request. This is also the reason
//!   `rspotify` is not a dependency despite TODO 7.5 naming it — its
//!   `tracks(ids)` / `albums(ids)` / `artists(ids)` conveniences *are* the batch
//!   endpoints, so using the crate would mean using exactly the code that cannot
//!   work. `ureq` directly is what `lyrics.rs` already uses and what a
//!   single-threaded TUI wants anyway.
//! - **The quota is per developer account and shared across Client IDs**, so
//!   there is no second app to fall back to and no way to buy more room. The
//!   [`Cache`] is therefore part of this task rather than an optimisation:
//!   [`CACHE_CAPACITY`] entries, least-recently-used out.
//! - **429 carries only `Retry-After`.** There is no `X-RateLimit-*` header and
//!   no quota endpoint to ask, so trak cannot show a "requests left" number and
//!   does not invent one. A 429 with `"reason": "QUOTA_EXCEEDED"` in the body is
//!   a *different* failure with the same status, and gets its own error variant.
//! - **A 403 has two real causes** and one of them is a sentence a person needs
//!   to read: the account is not on the app's five-user allowlist, or the
//!   endpoint is Premium-only (`POST /me/player/queue` is the one). Neither is
//!   "permission denied".
//! - **`Track.popularity` and `Album.popularity` are gone**, so no model here has
//!   a popularity field to show as a dash. The Info tab gets the real number
//!   from AppleScript, which is where it always came from.
//!
//! **Playback is not here.** Everything in this module is data; playing a result
//! is `play track "<uri>"` through the `Player` trait (SPEC §6), which works on
//! the Free tier and keeps COMPAT rule 3 intact. There is deliberately no
//! `POST /me/player/play` here.
//!
//! Two implementation notes, both about testing and both kept in the type:
//! [`SpotifyLibrary::at`] takes a base URL so every test in this file runs
//! against a loopback server or a fixture, and the 403 mapping is decided *per
//! endpoint* rather than globally, because the Premium-only 403 and the
//! allowlist 403 are the same status code and two different sentences.

use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::web::token::Token;

/// The production API root.
///
/// Not a parameter of every method on purpose: a track's `name` must never be
/// able to influence which host trak talks to, so this is a constant that
/// [`SpotifyLibrary::at`] replaces only in tests.
pub const API_BASE: &str = "https://api.spotify.com/v1";

/// How many results `GET /search` may ask for. `docs/WEB-API.md` §8 item 6:
/// "Search `limit` max is 10, default 5" — so this is a documented ceiling, not
/// a preference, and `search` never sends a larger one.
pub const SEARCH_LIMIT: u32 = 10;

/// What `GET /search` is asked for. The four groups are SPEC §6's grouped search
/// and TODO 7.6's four sections, and asking for fewer would leave a tab empty.
pub const SEARCH_TYPES: &str = "track,album,artist,playlist";

/// The longest `Retry-After` trak will believe.
///
/// Spotify publishes no rate-limit numbers at all (`docs/WEB-API.md` §4), so the
/// only honest bound is one trak chooses. Sixty seconds is under a worker
/// thread's patience if somebody tried to honour it literally, and long enough
/// that the next request is a real second later rather than the same request
/// again. Nothing here sleeps: the value is remembered as a *gate* — see
/// [`SpotifyLibrary::throttled`] — and the next call inside the window is
/// refused without touching the network.
pub const RETRY_AFTER_CAP: Duration = Duration::from_secs(60);

/// How many catalogue documents the cache holds.
///
/// Bounded because it is a `HashMap` inside a long-lived TUI and the quota is
/// per developer account (`docs/WEB-API.md` §2), so the useful thing is *fewer
/// requests*, not a bigger table. 256 covers a browsing session — an album page
/// pulls an album and a tracklist, an artist page pulls a page of albums — while
/// staying small enough that eviction is not worth a tree. Only the **catalogue**
/// is cached: documents Spotify's catalogue does not change. Anything the user
/// owns or edits (their playlists, their library, the queue, is-liked) is fetched
/// fresh every time, because a cached copy of it makes TODO 7.11's "remove from
/// a playlist" look like it did nothing.
pub const CACHE_CAPACITY: usize = 256;

/// How long one request may take. A hung API call must not hold a worker for the
/// rest of the session; the same bound `lyrics.rs` uses.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Refuse a body larger than this. The biggest thing trak asks for is a page of
/// search results or a library page; this is not one of those, and it should not
/// be a way to spend the owner's memory.
const MAX_BYTES: u64 = 4 * 1024 * 1024;

/// Say the same for every version, the way `lyrics.rs` and `web::auth` do.
const USER_AGENT: &str = concat!(
    "trak/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/Kathir-D/Trak)"
);

/// One image of a cover, from Spotify's `images` array.
///
/// The array is ordered largest-first by Spotify, and the width and height are
/// present but unused here: trak's `art` module needs a URL and does its own
/// resizing from the bytes.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Image {
    /// The `https` URL. Only Spotify's own image hosts are ever fetched from
    /// (`art.rs` enforces that), so a URL from a response is not a fetch order.
    pub url: String,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

/// An artist, in every place trak shows one.
///
/// `followers` and `popularity` are not here: `docs/WEB-API.md` §3 lists both
/// among the fields removed in dev mode, so there is nothing to model and
/// nothing to grey out.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Artist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub images: Vec<Image>,
}

/// An album.
///
/// One type for `GET /albums/{id}`, `GET /artists/{id}/albums` and the album
/// stub inside a search hit or a track, because they are the same thing with
/// different amounts filled in. The stub is why every field past `name` is
/// optional: a simplified album carries no release date and no track count, and
/// rejecting a response for that would make search results unshowable.
///
/// `popularity` and `album_group` and `label` are absent for the reason the
/// `Artist` doc comment gives.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Album {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub uri: String,
    /// `yyyy-mm-dd`, `yyyy-mm` or `yyyy`, exactly as Spotify sends it. Spotify's
    /// own string rather than a parsed date, because the precision genuinely
    /// varies and a tab that showed "unknown release date" for a 2011 album
    /// would be worse than a year.
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub artists: Vec<Artist>,
    #[serde(default)]
    pub images: Vec<Image>,
    /// Absent in a search hit and in the album object inside a track.
    #[serde(default)]
    pub total_tracks: Option<u32>,
}

/// A track, from search, from a tracklist, from a playlist or from the queue.
///
/// Playable by handing [`Track::uri`] to `Player::play_uri` — SPEC §6 keeps
/// playback on AppleScript so it works on Free — so `uri` is the field that
/// matters most here even though it is optional for a response that omits it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Track {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub duration_ms: u32,
    #[serde(default)]
    pub track_number: Option<u32>,
    #[serde(default)]
    pub disc_number: Option<u32>,
    #[serde(default)]
    pub artists: Vec<Artist>,
    /// Absent inside a search hit's track in some responses, so it is not
    /// required.
    #[serde(default)]
    pub album: Option<Album>,
}

impl Track {
    /// Whole seconds, matching `player::TrackInfo::duration_secs` so the History
    /// and Search tabs format a duration the same way.
    pub fn duration_secs(&self) -> u64 {
        u64::from(self.duration_ms) / 1000
    }
}

/// What a list row says. The *renderer* decides where it goes; this is the text
/// every search, playlist, library and history row wants, and it lives next to
/// the data so the tabs cannot each invent their own idea of a track's name.
/// The one-line form of an album, as a row.
///
/// `artists` first and the year after, because that is what identifies an album
/// to the person looking at it; a row with only a name is not enough to choose
/// between four of them.
impl fmt::Display for Album {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let artists: Vec<&str> = self.artists.iter().map(|a| a.name.as_str()).collect();
        match (artists.is_empty(), self.release_date.as_deref()) {
            (false, Some(date)) => {
                write!(f, "{} — {} ({})", self.name, artists.join(", "), year(date))
            }
            (false, None) => write!(f, "{} — {}", self.name, artists.join(", ")),
            (true, Some(date)) => write!(f, "{} ({})", self.name, year(date)),
            (true, None) => write!(f, "{}", self.name),
        }
    }
}

/// The four digits of an ISO date, or nothing for one trak cannot read. The year
/// is what a person scans a list for, and `2026-03-01` is twice the width for it.
fn year(date: &str) -> &str {
    let digits = date.chars().take_while(char::is_ascii_digit).count();
    if digits >= 4 { &date[..4] } else { "" }
}

/// The one-line form of an artist. No albumography and no follower count: both
/// were removed from dev mode, and a row padded with a zero would be a lie.
impl fmt::Display for Artist {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// The one-line form of a playlist, with its size when Spotify sent one.
impl fmt::Display for Playlist {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.contents.as_ref().and_then(|c| c.total) {
            Some(total) => write!(f, "{} · {} tracks", self.name, total),
            None => write!(f, "{}", self.name),
        }
    }
}

/// The one-line form of a library row: a track, or the episode row the Liked tab
/// and a playlist tracklist can both contain.
impl fmt::Display for TrackItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.track {
            Some(track) => write!(f, "{track}"),
            // A row with no track is a local file Spotify did not resolve, or an
            // entry the API has no body for. It is still a row and is still
            // selectable, so it needs something to read as.
            None => write!(f, "— unavailable —"),
        }
    }
}

impl fmt::Display for Track {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let artists: Vec<&str> = self.artists.iter().map(|a| a.name.as_str()).collect();
        match artists.is_empty() {
            // A response with no artists is not a reason to print the separator
            // with nothing beside it.
            true => f.write_str(&self.name),
            false => write!(f, "{} — {}", artists.join(", "), self.name),
        }
    }
}

/// One entry of a playlist or of the liked-songs list.
///
/// Both are `{"track": …}` entries inside an `items` array: the playlist wrapper
/// the rename in `docs/WEB-API.md` §3 describes is `items.items.item`, and
/// `GET /me/tracks` has the same shape. `track` is optional because a playlist
/// really can hold a podcast episode, and `GET /episodes` was removed in dev
/// mode — so an episode has nothing trak can fetch and is skipped rather than
/// invented. [`Page::tracks`] is what the tabs want.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TrackItem {
    #[serde(default)]
    pub track: Option<Track>,
}

/// The `items` object inside a playlist, for `GET /playlists/{id}`.
///
/// **This field is absent for any playlist the user does not own or collaborate
/// on** ("For other playlists, only metadata is returned and the `items` field
/// will be absent", `docs/WEB-API.md` §3). Nothing in trak may promise to show
/// an arbitrary playlist's tracks, so the type says `Option` and the tabs have
/// to cope with `None`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct PlaylistContents {
    #[serde(default)]
    pub total: Option<u32>,
    #[serde(default)]
    pub items: Vec<TrackItem>,
}

/// A playlist, from `GET /me/playlists` or `GET /playlists/{id}`.
///
/// `owner` is not modelled: the Playlists tab is the user's own (TODO 7.7), so
/// there is nobody else to name.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub uri: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub images: Vec<Image>,
    /// The renamed `tracks` field, absent for a playlist trak may not read.
    #[serde(default, rename = "items")]
    pub contents: Option<PlaylistContents>,
}

/// Where a page of a list ends, and where the next one starts.
///
/// Opaque on purpose. Spotify's `next` is a whole URL carrying its own
/// `offset` and `limit`, and trak follows it verbatim rather than inventing
/// paging parameters (`docs/WEB-API.md` names no page size for any list
/// endpoint, and `limit` is documented only for search). A caller therefore
/// never sees a cursor, an offset or a URL: it holds the value it was given and
/// hands it back, and cannot construct one. That is why the inner `String` is
/// private and has no accessor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Continuation(String);

/// One page of a list, and the continuation that follows it.
///
/// `next: None` is the end of the list. Every paged method takes
/// `after: Option<&Continuation>` and `None` means the first page, so a caller
/// never has to know which page it is on.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<Continuation>,
}

impl<T> Page<T> {
    /// An empty page with no continuation: what an empty list looks like, and
    /// what [`crate::web::api::Library`]'s fakes return for one.
    pub fn empty() -> Self {
        Self {
            items: Vec::new(),
            next: None,
        }
    }

    /// Whether there is nothing here and nothing after it.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.next.is_none()
    }
}

impl Page<TrackItem> {
    /// The entries that carry a track, skipping the ones that do not (a podcast
    /// episode, which trak cannot fetch — see [`TrackItem`]).
    pub fn tracks(&self) -> impl Iterator<Item = &Track> {
        self.items.iter().filter_map(|item| item.track.as_ref())
    }
}

/// What Spotify is about to play next, and after that.
///
/// Live state, never cached. The two field names come from Spotify's own
/// response; the rest of the TUI's queue tab is drawn from here.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Queue {
    #[serde(default, rename = "currently_playing")]
    pub now_playing: Option<Track>,
    #[serde(default, rename = "queue")]
    pub upcoming: Vec<Track>,
}

/// What `GET /search` found, in the four groups TODO 7.6 renders.
///
/// Capped at [`SEARCH_LIMIT`] per group and **not** paged: a live search follows
/// what is being typed, and a cursor for a query that changes on every keystroke
/// is a cursor nobody could use. The cap is the documented maximum, so asking
/// for more is not an option the API offers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchResults {
    pub tracks: Vec<Track>,
    pub albums: Vec<Album>,
    pub artists: Vec<Artist>,
    pub playlists: Vec<Playlist>,
}

impl SearchResults {
    /// True when nothing at all matched, which is what an empty state reads
    /// (`lyrics.rs`'s rule for a lookup that found nothing).
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
            && self.albums.is_empty()
            && self.artists.is_empty()
            && self.playlists.is_empty()
    }
}

/// Everything trak's Version A tabs ask Spotify for (TODO 7.6–7.13).
///
/// The shape is driven by the consumers rather than by the API, in three ways:
///
/// - **No playback.** A result is played by handing its URI to `Player`, so a
///   Free-tier user gets the same tabs as a Premium one.
/// - **Errors are one line each.** [`ApiError::notice`] exists because every one
///   of these failures reaches a toast, and a 403 in a stack trace is useless to
///   the person who has to fix it.
/// - **Paging is a value, not a state machine.** A caller passes back the
///   [`Continuation`] it was handed and never sees a cursor.
///
/// Methods take `&self` so an implementation can be shared as a `dyn Library`,
/// and so a cached read costs no mutable borrow at the call site.
pub trait Library {
    /// The four groups for `query`, at most [`SEARCH_LIMIT`] of each.
    ///
    /// An empty or whitespace-only query spends no request at all and returns an
    /// empty result: an empty `q` is a 400 the user did not ask for.
    fn search(&self, query: &str) -> Result<SearchResults, ApiError>;

    /// A page of the user's own playlists (TODO 7.7).
    fn playlists(&self, after: Option<&Continuation>) -> Result<Page<Playlist>, ApiError>;

    /// One playlist, including its `contents` when Spotify returns them.
    fn playlist(&self, id: &str) -> Result<Playlist, ApiError>;

    /// A page of a playlist's items. Absent `contents` on
    /// [`Library::playlist`] is the same fact seen from the other side, and this
    /// may fail the same way TODO 7.7 has to degrade on.
    fn playlist_items(
        &self,
        id: &str,
        after: Option<&Continuation>,
    ) -> Result<Page<TrackItem>, ApiError>;

    /// Create a playlist (TODO 7.11). `public: false` is the private default
    /// trak asks for: a personal tool should not publish by default.
    fn create_playlist(&self, name: &str, public: bool) -> Result<Playlist, ApiError>;

    /// Add one track to a playlist (TODO 7.11).
    fn add_to_playlist(&self, id: &str, uri: &str) -> Result<(), ApiError>;

    /// Remove one track from a playlist (TODO 7.11). `uri` is what trak must
    /// already know: `DELETE /playlists/{id}/items` is keyed by URI.
    fn remove_from_playlist(&self, id: &str, uri: &str) -> Result<(), ApiError>;

    /// A page of liked songs (TODO 7.7, `f` toggles).
    fn liked_tracks(&self, after: Option<&Continuation>) -> Result<Page<TrackItem>, ApiError>;

    /// Whether one track is liked, which is `GET /me/library/contains` and not
    /// the removed `GET /me/tracks/contains`.
    fn is_liked(&self, uri: &str) -> Result<bool, ApiError>;

    /// Like or unlike one track: `PUT`/`DELETE /me/library`, the consolidated
    /// endpoint that replaced `PUT`/`DELETE /me/tracks`.
    fn set_liked(&self, uri: &str, liked: bool) -> Result<(), ApiError>;

    /// What is playing and what is after it (TODO 7.8). Read-only, and not
    /// Premium-gated by Spotify's documentation.
    fn queue(&self) -> Result<Queue, ApiError>;

    /// Add one track to the queue (TODO 7.8's `A`).
    ///
    /// **Premium-only.** A 403 here is the expected Free-tier path, not a
    /// failure: it maps to [`ApiError::PremiumOnly`], which reads as a sentence
    /// rather than as an error.
    fn enqueue(&self, uri: &str) -> Result<(), ApiError>;

    /// A page of saved albums (TODO 7.9).
    fn saved_albums(&self, after: Option<&Continuation>) -> Result<Page<Album>, ApiError>;

    /// A page of followed artists (TODO 7.9).
    fn followed_artists(&self, after: Option<&Continuation>) -> Result<Page<Artist>, ApiError>;

    /// A page of recently played tracks, newest first (TODO 7.9). The timestamps
    /// in the response are not modelled: the list is already in order, and the
    /// doc does not name the field.
    fn recently_played(&self, after: Option<&Continuation>) -> Result<Page<Track>, ApiError>;

    /// A page of an artist's albums (TODO 7.10).
    ///
    /// **Albums only.** `GET /artists/{id}/top-tracks` was removed in dev mode
    /// with no replacement (`docs/WEB-API.md` §3), so the artist page is what
    /// this returns and the tab has to be built without top tracks.
    fn artist_albums(
        &self,
        artist: &str,
        after: Option<&Continuation>,
    ) -> Result<Page<Album>, ApiError>;

    /// One album's metadata (TODO 7.10). Its tracklist is
    /// [`Library::album_tracks`], because that is the endpoint there is.
    fn album(&self, id: &str) -> Result<Album, ApiError>;

    /// A page of an album's tracks (TODO 7.10).
    fn album_tracks(&self, id: &str, after: Option<&Continuation>)
    -> Result<Page<Track>, ApiError>;
}

/// Why a [`Library`] call failed, in terms trak can act on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    /// There is no token. Every tab that calls a [`Library`] has to check for
    /// this first, because it is the state before the guided setup and not a
    /// failure worth a red toast.
    #[error("Spotify is not connected yet")]
    NotConnected,
    /// HTTP 401. The access token was rejected, which in practice means the
    /// stored refresh token is spent and the answer is a new login, not a retry.
    #[error("the Spotify connection is no longer valid")]
    Unauthorized,
    /// HTTP 403 on anything but the queue: the account is not on the app's
    /// five-user allowlist, which is what Spotify documents happening when
    /// someone logs into a development-mode app they were not added to.
    #[error("this Spotify account is not on trak's developer allowlist")]
    NotAllowlisted,
    /// HTTP 403 on `POST /me/player/queue`: Spotify documents that endpoint as
    /// Premium-only, and this is the expected answer for a Free account.
    #[error("adding to the queue needs Spotify Premium")]
    PremiumOnly,
    /// HTTP 429. `retry_after` is the *capped* wait — the value Spotify sent
    /// clipped to [`RETRY_AFTER_CAP`] — and it is what trak holds the next
    /// request for.
    ///
    /// A rate limit is not the same as a spent quota: see
    /// [`ApiError::QuotaExceeded`].
    #[error("rate limited, retrying in {}s", .retry_after.as_secs())]
    RateLimited {
        /// The capped wait, which is also when trak will allow the next request.
        retry_after: Duration,
    },
    /// 429 with `"reason": "QUOTA_EXCEEDED"`: the development-mode quota for the
    /// whole developer account is spent. Since July 2026 that quota is shared
    /// across every Client ID (`docs/WEB-API.md` §2), so there is nothing trak
    /// can do but say so — and it is not a rate limit.
    #[error("the Spotify developer quota is spent; try again later")]
    QuotaExceeded,
    /// 404: no such id. A stale search result or a playlist that has gone.
    #[error("Spotify does not have that")]
    NotFound,
    /// The status said success and the body was not the JSON trak expected: a
    /// truncated body, a field that was required and is not there, or something
    /// that is not JSON at all.
    #[error("Spotify sent an answer trak could not read")]
    Malformed,
    /// Nothing is listening: no network, DNS failure, refused connection, or a
    /// body cut off mid-read.
    #[error("could not reach the Spotify Web API")]
    Unreachable,
    /// No answer within [`REQUEST_TIMEOUT`].
    #[error("the Spotify Web API did not answer in time")]
    Timeout,
    /// A status trak does not act on. Kept rather than folded into "something
    /// went wrong" so a new one is visible in a toast.
    #[error("Spotify answered HTTP {0}")]
    Status(u16),
}

impl ApiError {
    /// One line, for a toast. Never a newline, never a stack trace, never a
    /// serde message — and for the three cases a user can fix, never just the
    /// status code.
    pub fn notice(&self) -> String {
        match self {
            ApiError::NotConnected => {
                "spotify: not connected yet — run the Spotify setup in trak config".to_string()
            }
            ApiError::Unauthorized => {
                "spotify: the connection has expired — log in again".to_string()
            }
            ApiError::NotAllowlisted => {
                "spotify: this account is not on trak's 5-user developer allowlist".to_string()
            }
            ApiError::PremiumOnly => {
                "spotify: adding to the queue needs Spotify Premium".to_string()
            }
            ApiError::RateLimited { retry_after } => {
                format!(
                    "spotify: rate limited, retrying in {}s",
                    retry_after.as_secs()
                )
            }
            ApiError::QuotaExceeded => {
                "spotify: the developer quota is spent — try again later".to_string()
            }
            ApiError::NotFound => "spotify: not found".to_string(),
            ApiError::Malformed => "spotify: sent an answer trak could not read".to_string(),
            ApiError::Unreachable => "spotify: could not reach the Spotify Web API".to_string(),
            ApiError::Timeout => "spotify: the Web API did not answer in time".to_string(),
            ApiError::Status(code) => format!("spotify: answered HTTP {code}"),
        }
    }

    /// Whether the next thing to do is a fresh login rather than another try.
    ///
    /// Only the two cases where nothing trak can do makes the answer different:
    /// a rejected token, and an account Spotify will not serve. A rate limit, a
    /// transport failure and a malformed body are all worth asking again.
    pub fn needs_relogin(&self) -> bool {
        matches!(self, ApiError::Unauthorized | ApiError::NotAllowlisted)
    }

    /// Whether trying again in a moment is the thing to do. The inverse of
    /// [`ApiError::needs_relogin`] for the cases where both questions matter,
    /// and the reason a toast can say "retrying" without being wrong.
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            ApiError::RateLimited { .. }
                | ApiError::QuotaExceeded
                | ApiError::Unreachable
                | ApiError::Timeout
                | ApiError::Status(_)
        )
    }
}

// ---------------------------------------------------------------------------
// SpotifyLibrary
// ---------------------------------------------------------------------------

/// Which HTTP verb a call uses.
///
/// An enum rather than a string because a misspelled verb should be a compile
/// error, and because `ureq` exposes one method per verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }
}

/// One answer, before anything has decided what it means.
struct Reply {
    status: u16,
    body: String,
    /// Clipped to [`RETRY_AFTER_CAP`].
    retry_after: Option<Duration>,
}

/// The real [`Library`]: the Web API over `ureq`.
///
/// Blocking on purpose, like `web::auth` and `lyrics.rs` — this runs on a worker
/// thread behind a trait, and an async client would drag a runtime into a TUI
/// that has none.
pub struct SpotifyLibrary {
    base: String,
    /// The bearer token, behind a lock because an access token lasts one hour
    /// and `web::auth::Session` renews it underneath a library the TUI already
    /// holds. Rebuilding the object to change a token would mean the TUI holding
    /// a `Option<SpotifyLibrary>` it has to re-plumb; a setter does not.
    token: Mutex<String>,
    agent: ureq::Agent,
    cache: Mutex<Cache>,
    /// When the next request is allowed. `None` means now.
    ///
    /// This is how a 429 is honoured without ever sleeping: the worker is not
    /// the place to block, so the wait is remembered here and enforced by
    /// refusing every call inside the window without touching the network.
    throttled: Mutex<Option<Instant>>,
    retry_cap: Duration,
}

impl SpotifyLibrary {
    /// A library against the real API, with the token `web::auth` produced.
    pub fn new(token: &Token) -> Self {
        Self::at(API_BASE, token)
    }

    /// A library against a named base URL. The only concession to testability
    /// here, and the reason every fixture test in this file can point the client
    /// at a loopback server; production never passes anything but [`API_BASE`].
    pub fn at(base: &str, token: &Token) -> Self {
        Self::build(base, token.access_token(), REQUEST_TIMEOUT, RETRY_AFTER_CAP)
    }

    fn build(base: &str, token: &str, timeout: Duration, retry_cap: Duration) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            // A 403 and a 429 are answers with meaning, not exceptions: ureq's
            // default throws the body away and hands back only the code, and the
            // body is where `Retry-After`'s companion `reason` lives.
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .build()
            .new_agent();
        Self {
            base: base.trim_end_matches('/').to_string(),
            token: Mutex::new(token.to_string()),
            agent,
            cache: Mutex::new(Cache::new()),
            throttled: Mutex::new(None),
            retry_cap,
        }
    }

    /// A test-double API: builders that only the tests in this file use.
    #[allow(
        dead_code,
        reason = "test-double API: builders and accessors are used by tests, not by trak"
    )]
    pub fn with_retry_cap(mut self, cap: Duration) -> Self {
        self.retry_cap = cap;
        self
    }

    /// A test-double API: how long one request may take. The loopback tests need
    /// a fraction of [`REQUEST_TIMEOUT`] to cover the timeout path, and the
    /// agent is rebuilt because a timeout is a construction-time setting.
    #[allow(
        dead_code,
        reason = "test-double API: builders and accessors are used by tests, not by trak"
    )]
    pub fn with_timeout(self, timeout: Duration) -> Self {
        let token = lock(&self.token).clone();
        let mut rebuilt = Self::build(&self.base, &token, timeout, self.retry_cap);
        // Whatever has already been fetched stays fetched: a test that shortens
        // the timeout must not silently lose the cache it was measuring.
        rebuilt.cache = Mutex::new(lock(&self.cache).clone());
        rebuilt.throttled = Mutex::new(*lock(&self.throttled));
        rebuilt
    }

    /// Take a renewed access token. Called by whatever holds the
    /// [`web::auth::Session`](crate::web::auth::Session) after a refresh; a
    /// cached catalogue document stays valid across it, because a token is not
    /// part of the key.
    pub fn set_token(&self, token: &Token) {
        *lock(&self.token) = token.access_token().to_string();
    }

    /// How many documents the cache is holding. A test-double accessor.
    #[allow(
        dead_code,
        reason = "test-double API: builders and accessors are used by tests, not by trak"
    )]
    pub fn cached_len(&self) -> usize {
        lock(&self.cache).len()
    }

    /// The full URL for a path and query, built here so every request in this
    /// file is one line and the encoding is in one place.
    fn url(&self, path: &str, query: &[(&str, String)]) -> String {
        let mut url = format!("{}{path}", self.base);
        if !query.is_empty() {
            url.push('?');
            let encoded: Vec<String> = query
                .iter()
                .map(|(key, value)| format!("{}={}", percent_encode(key), percent_encode(value)))
                .collect();
            url.push_str(&encoded.join("&"));
        }
        url
    }

    /// The URL a continuation stands for, or [`ApiError::Malformed`].
    ///
    /// Spotify's `next` is an absolute URL that arrives inside a response body,
    /// so it is followed only if it is *this* API. A `next` pointing anywhere
    /// else — a proxy, a mirror, a body trak did not expect — is refused rather
    /// than requested with the user's bearer token attached, which is the whole
    /// reason this check exists.
    fn continuation_url(&self, next: &Continuation) -> Result<String, ApiError> {
        let Some(path) = next.0.strip_prefix(&self.base) else {
            return Err(ApiError::Malformed);
        };
        // The base on its own is not a request, so the remainder has to start the
        // path. Without this a `next` of `<base>v1/...` -- which no server sends,
        // and which a hostile body could -- would build a URL trak requested.
        if !path.starts_with('/') {
            return Err(ApiError::Malformed);
        }
        Ok(path.to_string())
    }

    /// Refuse everything while a 429 window is open, without a request.
    fn gate(&self) -> Result<(), ApiError> {
        let until = *lock(&self.throttled);
        match until {
            Some(at) => {
                let now = Instant::now();
                if now < at {
                    return Err(ApiError::RateLimited {
                        retry_after: at - now,
                    });
                }
                Ok(())
            }
            None => Ok(()),
        }
    }

    /// One request, with the checks that have to happen before a body is parsed.
    fn send(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&str>,
        premium_only: bool,
    ) -> Result<Reply, ApiError> {
        self.gate()?;
        let url = self.url(path, query);
        let bearer = format!("Bearer {}", lock(&self.token).clone());
        let json = body.map(|json| json.to_string());
        let sent = match (method, json) {
            (Method::Get, _) => self.agent.get(&url).header("Authorization", bearer).call(),
            (Method::Post, Some(json)) => self
                .agent
                .post(&url)
                .header("Authorization", bearer)
                .content_type("application/json")
                .send(json),
            (Method::Post, None) => self
                .agent
                .post(&url)
                .header("Authorization", bearer)
                .send_empty(),
            (Method::Put, Some(json)) => self
                .agent
                .put(&url)
                .header("Authorization", bearer)
                .content_type("application/json")
                .send(json),
            (Method::Put, None) => self
                .agent
                .put(&url)
                .header("Authorization", bearer)
                .send_empty(),
            // `DELETE` is a no-body verb in ureq until a body is forced, and the
            // playlist-item removal has one.
            (Method::Delete, Some(json)) => self
                .agent
                .delete(&url)
                .force_send_body()
                .header("Authorization", bearer)
                .content_type("application/json")
                .send(json),
            (Method::Delete, None) => self
                .agent
                .delete(&url)
                .header("Authorization", bearer)
                .call(),
        };
        let mut response = sent.map_err(transport)?;
        let status = response.status().as_u16();
        let retry_after = retry_after(response.headers(), self.retry_cap);
        // A declared length over the cap is refused before a byte is read. The
        // read limit alone would do it too, but it does it by failing the read,
        // and a failed read is indistinguishable from a network that went away --
        // so the user would be told to check their connection because a response
        // was too large. Refusing up front also means a hostile or runaway
        // response is never pulled into memory at all.
        if let Some(len) = declared_length(response.headers())
            && len > MAX_BYTES
        {
            return Err(ApiError::Malformed);
        }
        // One byte over the cap, so a *chunked* answer with no declared length
        // arrives and is rejected by `decode` rather than failing the read.
        let bytes = response
            .body_mut()
            .with_config()
            .limit(MAX_BYTES + 1)
            .read_to_string()
            .map_err(transport)?;
        let reply = Reply {
            status,
            body: bytes,
            retry_after,
        };
        self.check(&reply, premium_only)?;
        Ok(reply)
    }

    /// Turn a status into an [`ApiError`] or into permission to read the body.
    ///
    /// The 403 split is the reason this takes a flag: the same status is
    /// "your account is not allowlisted" on almost every endpoint and "that is
    /// Premium" on `POST /me/player/queue`, and the user fixes them differently.
    fn check(&self, reply: &Reply, premium_only: bool) -> Result<(), ApiError> {
        match reply.status {
            200..=299 => return Ok(()),
            401 => return Err(ApiError::Unauthorized),
            403 => {
                return Err(if premium_only {
                    ApiError::PremiumOnly
                } else {
                    ApiError::NotAllowlisted
                });
            }
            404 => return Err(ApiError::NotFound),
            429 => return Err(self.throttle(reply)),
            _ => {}
        }
        Err(ApiError::Status(reply.status))
    }

    /// Record the 429 window and decide which of the two 429s this is.
    ///
    /// Both hold the next request off for the same capped wait, because in both
    /// cases asking again immediately is the one thing that is certain to fail.
    /// The *notice* differs, since a rate limit clears on its own and a spent
    /// development quota does not.
    fn throttle(&self, reply: &Reply) -> ApiError {
        let wait = reply.retry_after.unwrap_or(self.retry_cap);
        let until = match Instant::now().checked_add(wait) {
            Some(at) => at,
            // A wait past the representable range cannot be held, so no window
            // is recorded. `retry_cap` keeps this unreachable in practice; the
            // branch exists because `Instant + Duration` panics on overflow and
            // a panic in a worker is worse than a missed back-off.
            None => return ApiError::RateLimited { retry_after: wait },
        };
        *lock(&self.throttled) = Some(until);
        if quota_exceeded(&reply.body) {
            ApiError::QuotaExceeded
        } else {
            ApiError::RateLimited { retry_after: wait }
        }
    }

    /// One GET, decoded, with no caching.
    fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, ApiError> {
        let reply = self.send(Method::Get, path, query, None, false)?;
        decode(&reply.body)
    }

    /// One write with a JSON body, for the endpoint that answers with the thing
    /// it created (`POST /me/playlists` is the only one).
    fn send_json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: &str,
        premium_only: bool,
    ) -> Result<T, ApiError> {
        let reply = self.send(method, path, query, Some(body), premium_only)?;
        decode(&reply.body)
    }

    /// One write whose response body trak has no use for. Spotify answers
    /// `200`/`201`/`204` with nothing, which is the good case.
    fn send_ok(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<&str>,
        premium_only: bool,
    ) -> Result<(), ApiError> {
        self.send(method, path, query, body, premium_only)
            .map(|_| ())
    }

    /// [`Self::get_json`] behind the cache.
    ///
    /// The key is the method plus the path and query, so it is the request's
    /// identity: two calls that would send the same bytes share an entry, and a
    /// different page of a different list can never collide with one.
    fn cached_json<T: Clone + DeserializeOwned + Send + Sync + 'static>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<T, ApiError> {
        let key = cache_key(Method::Get, path, query);
        if let Some(hit) = lock(&self.cache).get::<T>(&key) {
            return Ok(hit);
        }
        let value: T = self.get_json(path, query)?;
        lock(&self.cache).put(key, value.clone());
        Ok(value)
    }

    /// One page of a list: the URL to ask is either the first page (no paging
    /// parameters at all — `docs/WEB-API.md` names a `limit` for search only) or
    /// the continuation handed back last time, followed verbatim.
    fn page<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
        after: Option<&Continuation>,
    ) -> Result<Page<T>, ApiError> {
        let url = match after {
            Some(next) => self.continuation_url(next)?,
            None => self.url(path, query),
        };
        let reply = self.send(Method::Get, &url, &[], None, false)?;
        let wire: Paged<T> = decode(&reply.body)?;
        Ok(Page {
            items: wire.items,
            next: wire.next.map(Continuation),
        })
    }

    /// The body of a library write. `ids` is a single-element array and
    /// `types` says what it is, because the consolidated endpoint replaced
    /// `/me/tracks` and `/me/following` with one call that has to be told.
    fn library_body(uri: &str, kind: &str) -> String {
        format!(r#"{{"ids":["{uri}"],"types":["{kind}"]}}"#)
    }
}

/// A paged response, before its `next` is checked against the base URL.
#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "T: DeserializeOwned"))]
struct Paged<T> {
    #[serde(default)]
    items: Vec<T>,
    #[serde(default)]
    next: Option<String>,
}

/// The `items` wrapper a search response puts around each group.
#[derive(Debug, Clone, Deserialize)]
#[serde(bound(deserialize = "T: DeserializeOwned"))]
struct Group<T> {
    #[serde(default)]
    items: Vec<T>,
}

/// The four groups as they arrive, so [`SearchResults`] can be trak's shape and
/// not Spotify's.
#[derive(Debug, Clone, Deserialize)]
struct SearchBody {
    tracks: Option<Group<Track>>,
    albums: Option<Group<Album>>,
    artists: Option<Group<Artist>>,
    playlists: Option<Group<Playlist>>,
}

/// A search response with one group missing entirely must still read as an
/// empty one; that is what `Option` is for, and it is a real case: a query that
/// matches no playlist still gets a `playlists` key, but a 200 with one group
/// removed should not take the whole result away.
fn groups<T>(group: Option<Group<T>>) -> Vec<T> {
    group.map_or_else(Vec::new, |group| group.items)
}

/// `{"error": {"reason": "QUOTA_EXCEEDED"}}`, and nothing else from the error
/// object.
///
/// Parsed rather than substring-matched because the word also appears in prose
/// and in `message`, and the *reason* field is the one the doc points at: "check
/// the body for `\"reason\": \"QUOTA_EXCEEDED\"`" (§4).
fn quota_exceeded(body: &str) -> bool {
    #[derive(Deserialize)]
    struct Envelope {
        error: Option<Reason>,
    }
    #[derive(Deserialize)]
    struct Reason {
        reason: Option<String>,
    }
    serde_json::from_str::<Envelope>(body).is_ok_and(|envelope| {
        envelope
            .error
            .is_some_and(|error| error.reason.as_deref() == Some("QUOTA_EXCEEDED"))
    })
}

/// The `Retry-After` header, clipped.
///
/// `docs/WEB-API.md` §4: "The header of the 429 response will normally include
/// a `Retry-After` header with a value in seconds." "Normally" is why a missing
/// one has an answer of its own — the caller's default wait — rather than being
/// read as zero, which would retry immediately and make the 429 worse.
/// The `Content-Length` a response declares, if it declares one at all.
///
/// A chunked response has none, which is not an error: the read limit and
/// `decode` cover that case. This is only the cheap pre-check.
fn declared_length(headers: &ureq::http::HeaderMap) -> Option<u64> {
    headers
        .get("content-length")?
        .to_str()
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn retry_after(headers: &ureq::http::HeaderMap, cap: Duration) -> Option<Duration> {
    let value = headers.get("retry-after")?.to_str().ok()?;
    let seconds: u64 = value.trim().parse().ok()?;
    Some(Duration::from_secs(seconds).min(cap))
}

/// An `ureq` failure as one of trak's.
///
/// The `StatusCode` arm is unreachable with `http_status_as_error(false)` and is
/// kept so that turning the setting back on does not turn every 403 into a
/// transport failure.
fn transport(e: ureq::Error) -> ApiError {
    match e {
        ureq::Error::Timeout(_) => ApiError::Timeout,
        ureq::Error::StatusCode(_) => ApiError::Unreachable,
        _ => ApiError::Unreachable,
    }
}

/// A body as the type the caller asked for, or [`ApiError::Malformed`].
///
/// The one place a truncated document, a missing required field and something
/// that was never JSON all become the same error, which is right: trak has no
/// answer for any of them and the toast is the same either way.
fn decode<T: DeserializeOwned>(body: &str) -> Result<T, ApiError> {
    if body.len() as u64 > MAX_BYTES {
        return Err(ApiError::Malformed);
    }
    serde_json::from_str(body).map_err(|_| ApiError::Malformed)
}

/// The key a cached response is filed under: the verb, the path and the query,
/// which together *are* the request.
fn cache_key(method: Method, path: &str, query: &[(&str, String)]) -> String {
    let mut key = format!("{} {path}", method.as_str());
    if !query.is_empty() {
        key.push('?');
        let encoded: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{}={}", percent_encode(name), percent_encode(value)))
            .collect();
        key.push_str(&encoded.join("&"));
    }
    key
}

/// A bounded cache of catalogue documents, keyed by request and evicted
/// least-recently-used.
///
/// The value is a type-erased clone rather than a `T`, because one cache holds
/// four different response types and the key already says which is which. A read
/// touches the entry's clock so the order is *used* order, not inserted order:
/// a cache that evicted by age would throw away the album page the user is
/// looking at when they back into the search that found it.
#[derive(Clone)]
struct Cache {
    entries: HashMap<String, (Arc<dyn Any + Send + Sync>, u64)>,
    clock: u64,
}

impl Cache {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            clock: 0,
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn get<T: Clone + Send + Sync + 'static>(&mut self, key: &str) -> Option<T> {
        let clock = self.tick();
        let (value, used) = self.entries.get_mut(key)?;
        *used = clock;
        value.downcast_ref::<T>().cloned()
    }

    fn put<T: Clone + Send + Sync + 'static>(&mut self, key: String, value: T) {
        let clock = self.tick();
        // Insert first, then make room: doing it the other way round would evict
        // the entry that was just inserted when the cache is exactly full.
        self.entries
            .insert(key, (Arc::new(value) as Arc<dyn Any + Send + Sync>, clock));
        self.evict_to(CACHE_CAPACITY);
    }

    fn tick(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }

    fn evict_to(&mut self, bound: usize) {
        while self.entries.len() > bound {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| key.clone())
            else {
                return;
            };
            self.entries.remove(&oldest);
        }
    }
}

/// A lock that recovers from poisoning.
///
/// A panic on a worker thread poisons whatever mutex it held, and a TUI that
/// answers every later call with a panic is a TUI that does not come back. The
/// data behind these locks is a cache, a token and a deadline — all of which are
/// plain values, so taking them out of a poisoned lock is safe even though the
/// thread that wrote one died.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Library for SpotifyLibrary {
    fn search(&self, query: &str) -> Result<SearchResults, ApiError> {
        let query = query.trim();
        if query.is_empty() {
            // An empty `q` is a request trak cannot make a useful answer from,
            // and this is the one place the debounce can fire before a single
            // character is typed.
            return Ok(SearchResults::default());
        }
        let query = vec![
            ("q", query.to_string()),
            ("type", SEARCH_TYPES.to_string()),
            ("limit", SEARCH_LIMIT.to_string()),
        ];
        let body: SearchBody = self.cached_json("/search", &query)?;
        Ok(SearchResults {
            tracks: groups(body.tracks),
            albums: groups(body.albums),
            artists: groups(body.artists),
            playlists: groups(body.playlists),
        })
    }

    fn playlists(&self, after: Option<&Continuation>) -> Result<Page<Playlist>, ApiError> {
        self.page("/me/playlists", &[], after)
    }

    /// Deliberately uncached. `GET /playlists/{id}` is a playlist the user may
    /// own and rename (TODO 7.11 creates one), and trak cannot tell an owned
    /// playlist from a borrowed one without reading the owner -- so it caches
    /// neither.
    fn playlist(&self, id: &str) -> Result<Playlist, ApiError> {
        self.get_json(&format!("/playlists/{}", segment(id)), &[])
    }

    fn playlist_items(
        &self,
        id: &str,
        after: Option<&Continuation>,
    ) -> Result<Page<TrackItem>, ApiError> {
        self.page(&format!("/playlists/{}/items", segment(id)), &[], after)
    }

    fn create_playlist(&self, name: &str, public: bool) -> Result<Playlist, ApiError> {
        // The name goes in as a JSON string, so it is escaped rather than
        // quoted into a template: a playlist named `he said "hi"` is ordinary.
        let body = serde_json::json!({ "name": name, "public": public }).to_string();
        self.send_json(Method::Post, "/me/playlists", &[], &body, false)
    }

    fn add_to_playlist(&self, id: &str, uri: &str) -> Result<(), ApiError> {
        // One URI per request, for the same reason everything else is one id per
        // request: the batch endpoints are gone and the quota is shared.
        self.send_ok(
            Method::Post,
            &format!("/playlists/{}/items", segment(id)),
            &[("uris", uri.to_string())],
            None,
            false,
        )
    }

    fn remove_from_playlist(&self, id: &str, uri: &str) -> Result<(), ApiError> {
        self.send_ok(
            Method::Delete,
            &format!("/playlists/{}/items", segment(id)),
            &[],
            Some(&format!(r#"{{"tracks":[{{"uri":"{uri}"}}]}}"#)),
            false,
        )
    }

    fn liked_tracks(&self, after: Option<&Continuation>) -> Result<Page<TrackItem>, ApiError> {
        self.page("/me/tracks", &[], after)
    }

    fn is_liked(&self, uri: &str) -> Result<bool, ApiError> {
        let flags: Vec<bool> = self.get_json(
            "/me/library/contains",
            &[("ids", uri.to_string()), ("types", "track".to_string())],
        )?;
        // One id in, one boolean out. An empty array means the answer to the
        // question trak asked is not in the answer, which is a broken response
        // rather than an answer of "no": a liked song that reads as unliked
        // makes `f` save over itself.
        flags.into_iter().next().ok_or(ApiError::Malformed)
    }

    fn set_liked(&self, uri: &str, liked: bool) -> Result<(), ApiError> {
        self.send_ok(
            if liked { Method::Put } else { Method::Delete },
            "/me/library",
            &[],
            Some(&Self::library_body(uri, "track")),
            false,
        )
    }

    fn queue(&self) -> Result<Queue, ApiError> {
        self.get_json("/me/player/queue", &[])
    }

    fn enqueue(&self, uri: &str) -> Result<(), ApiError> {
        self.send_ok(
            Method::Post,
            "/me/player/queue",
            &[("uri", uri.to_string())],
            None,
            // The one endpoint whose 403 means "Premium", and the reason a Free
            // user's `A` is a sentence rather than an error.
            true,
        )
    }

    fn saved_albums(&self, after: Option<&Continuation>) -> Result<Page<Album>, ApiError> {
        self.page("/me/albums", &[], after)
    }

    fn followed_artists(&self, after: Option<&Continuation>) -> Result<Page<Artist>, ApiError> {
        // `type` is the endpoint's own required parameter, not a preference:
        // `/me/following` without it is a 400. `artist` is the only type trak has
        // a tab for, since podcasts and shows were not asked for.
        self.page("/me/following", &[("type", "artist".to_string())], after)
    }

    fn recently_played(&self, after: Option<&Continuation>) -> Result<Page<Track>, ApiError> {
        let items: Page<TrackItem> = self.page("/me/player/recently-played", &[], after)?;
        // The response wraps each row in `{"track": …}`; the tab wants tracks,
        // and a podcast episode in the list has nothing trak can fetch.
        Ok(Page {
            items: items
                .items
                .into_iter()
                .filter_map(|item| item.track)
                .collect(),
            next: items.next,
        })
    }

    fn artist_albums(
        &self,
        artist: &str,
        after: Option<&Continuation>,
    ) -> Result<Page<Album>, ApiError> {
        self.page(&format!("/artists/{}/albums", segment(artist)), &[], after)
    }

    fn album(&self, id: &str) -> Result<Album, ApiError> {
        self.cached_json(&format!("/albums/{}", segment(id)), &[])
    }

    fn album_tracks(
        &self,
        id: &str,
        after: Option<&Continuation>,
    ) -> Result<Page<Track>, ApiError> {
        let items: Page<TrackItem> =
            self.page(&format!("/albums/{}/tracks", segment(id)), &[], after)?;
        Ok(Page {
            items: items
                .items
                .into_iter()
                .filter_map(|item| item.track)
                .collect(),
            next: items.next,
        })
    }
}

// ---------------------------------------------------------------------------
// FakeLibrary
// ---------------------------------------------------------------------------

/// The data a [`FakeLibrary`] holds.
#[derive(Debug, Default)]
struct FakeData {
    tracks: Vec<Track>,
    albums: Vec<Album>,
    artists: Vec<Artist>,
    playlists: Vec<Playlist>,
    /// A playlist's contents, by playlist id. Empty for a playlist trak may not
    /// read, which is how the "items is absent" case is reproducible.
    contents: HashMap<String, Vec<TrackItem>>,
    liked: Vec<String>,
    queue: Vec<Track>,
    saved_albums: Vec<Album>,
    followed: Vec<Artist>,
    recent: Vec<Track>,
    /// Set by [`FakeLibrary::failing`]; every method returns it.
    error: Option<ApiError>,
    /// Makes `create_playlist` deterministic: the id is `fake-playlist-1`, then
    /// `-2`, so a test can assert on the whole playlist it was handed.
    created: u64,
}

/// How many items the fake puts in one page. Arbitrary but fixed: the real page
/// size is whatever Spotify decides, and trak must not care.
const FAKE_PAGE: usize = 20;

/// An in-memory [`Library`], so the whole of Phase 7 runs with no network, no
/// Spotify and no token (TODO 7.6–7.13's done-when is `update()` tests with this
/// type).
///
/// It is also where the *shape* of trak's Version A is pinned down, because a
/// fake that returns something subtly different from the real client is a fake
/// that makes the tabs pass against data the API never sends. So it returns the
/// same types, holds the same rules — one page at a time, an empty query costs
/// nothing, a playlist trak does not own has no `contents` — and records every
/// write so a test can prove a read did not write.
pub struct FakeLibrary {
    data: Mutex<FakeData>,
    writes: Mutex<Vec<&'static str>>,
}

// A test double's builders and accessors are its public API. Most are only
// reached from tests, so the release build sees them as unused.
#[allow(
    dead_code,
    reason = "test-double API: builders and accessors are used by tests, not by trak"
)]
impl FakeLibrary {
    /// An empty library.
    pub fn new() -> Self {
        Self {
            data: Mutex::new(FakeData::default()),
            writes: Mutex::new(Vec::new()),
        }
    }

    /// A library that fails every call with `error`, for the toast paths.
    pub fn failing(error: ApiError) -> Self {
        let library = Self::new();
        lock(&library.data).error = Some(error);
        library
    }

    /// Seed the catalogue. Search filters over everything seeded here.
    pub fn with_catalogue(
        tracks: Vec<Track>,
        albums: Vec<Album>,
        artists: Vec<Artist>,
        playlists: Vec<Playlist>,
    ) -> Self {
        let library = Self::new();
        {
            let mut data = lock(&library.data);
            data.tracks = tracks;
            data.albums = albums;
            data.artists = artists;
            data.playlists = playlists;
        }
        library
    }

    /// A library seeded with a small, fixed Massive Attack catalogue, so the
    /// binary's hidden `--fake-library` flag has something to find with no
    /// network, no token and no Spotify. `player::sample_track` is the same
    /// idea one layer down: one realistic state every test shares, so an
    /// end-to-end test asserts against data it did not invent on the spot.
    ///
    /// The values are the ones the fixtures in this file are written from,
    /// hand-written rather than shared with the test module for the reason that
    /// module's own header gives: a fake seeded by the builders under test
    /// would agree with them whatever both got wrong. One entry is *not* from
    /// a fixture: the song "Mezzanine", which the album of the same name also
    /// contains, so a `play mezzanine` has a name match that is not the first
    /// row — the one case that tells the best-match rule from a plain
    /// "first result".
    pub fn seeded() -> Self {
        let artist = Artist {
            id: "4Z8W4fKeB5YxbusRsdQVPb".to_string(),
            name: "Massive Attack".to_string(),
            uri: "spotify:artist:4Z8W4fKeB5YxbusRsdQVPb".to_string(),
            images: Vec::new(),
        };
        let album = Album {
            id: "5nMdc39z78kifAc5WXv9Yj".to_string(),
            name: "Mezzanine".to_string(),
            uri: "spotify:album:5nMdc39z78kifAc5WXv9Yj".to_string(),
            release_date: Some("1998-04-20".to_string()),
            artists: vec![artist.clone()],
            images: Vec::new(),
            total_tracks: Some(11),
        };
        let on = |id: &str, name: &str, number: u32| Track {
            id: id.to_string(),
            name: name.to_string(),
            uri: format!("spotify:track:{id}"),
            duration_ms: 0,
            track_number: Some(number),
            disc_number: Some(1),
            artists: vec![artist.clone()],
            album: Some(album.clone()),
        };
        let tracks = vec![
            on("6HacgXCExkzS552ILfJTXu", "Teardrop", 10),
            on("5ghIJDpP6863d6KFwdPhJ3", "Angel", 1),
            on("2pKZIvNQC1codGnMYCjBqY", "Mezzanine", 3),
        ];
        let playlist = Playlist {
            id: "37i9dQZF1DXcBWIGoYBM5M".to_string(),
            name: "Massive Attack on Repeat".to_string(),
            uri: "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M".to_string(),
            description: Some("Long drives".to_string()),
            images: Vec::new(),
            contents: Some(PlaylistContents {
                total: Some(3),
                items: Vec::new(),
            }),
        };
        Self::with_catalogue(tracks, vec![album], vec![artist], vec![playlist])
    }

    /// Seed the user's library and playback state (TODO 7.7–7.9).
    pub fn with_library(
        self,
        liked: Vec<Track>,
        queue: Vec<Track>,
        saved_albums: Vec<Album>,
        followed: Vec<Artist>,
        recent: Vec<Track>,
    ) -> Self {
        {
            let mut data = lock(&self.data);
            data.liked = liked.iter().map(|track| track.uri.clone()).collect();
            data.queue = queue;
            data.saved_albums = saved_albums;
            data.followed = followed;
            data.recent = recent;
        }
        self
    }

    /// Make a playlist's contents readable, the way `GET /playlists/{id}` returns
    /// them for a playlist the user owns.
    pub fn with_contents(self, playlist: &str, tracks: Vec<Track>) -> Self {
        lock(&self.data).contents.insert(
            playlist.to_string(),
            tracks
                .into_iter()
                .map(|track| TrackItem { track: Some(track) })
                .collect(),
        );
        self
    }

    /// Like a track without going through [`Library::set_liked`], so a test can
    /// set up the liked list without recording a write.
    pub fn seed_liked(&self, uris: &[&str]) {
        lock(&self.data).liked = uris.iter().map(|uri| (*uri).to_string()).collect();
    }

    /// What every call should fail with from now on, or `None` to stop failing.
    pub fn set_error(&self, error: Option<ApiError>) {
        lock(&self.data).error = error;
    }

    /// Every write attempted, in order, so a test can prove a read is read-only
    /// and that `f` wrote exactly once.
    pub fn writes(&self) -> Vec<&'static str> {
        lock(&self.writes).clone()
    }

    /// Forget the recorded writes.
    pub fn clear_writes(&self) {
        lock(&self.writes).clear();
    }
}

impl Default for FakeLibrary {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeLibrary {
    /// The injected error, if there is one. Checked at the top of every method
    /// so a test can drive the toast paths without a server.
    fn gate(&self) -> Result<(), ApiError> {
        match lock(&self.data).error.clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn record(&self, what: &'static str) {
        lock(&self.writes).push(what);
    }

    /// One page out of `items`, after `after`.
    fn page<T: Clone>(&self, items: &[T], after: Option<&Continuation>) -> Page<T> {
        let start: usize = match after {
            Some(next) => next.0.parse().unwrap_or(0),
            None => 0,
        };
        let rest = items.get(start..).unwrap_or(&[]);
        let taken: Vec<T> = rest.iter().take(FAKE_PAGE).cloned().collect();
        let after_page = start + taken.len();
        Page {
            items: taken,
            next: (after_page < items.len()).then(|| Continuation(after_page.to_string())),
        }
    }

    /// A case-insensitive substring match over a track, the same shape as the
    /// Spotify search the fake stands in for: the title, the artist or the album.
    /// Nothing cleverer is wanted, because a fake that ranked results would be
    /// testing its own ranking.
    fn matches(needle: &str, track: &Track) -> bool {
        let needle = needle.to_lowercase();
        let in_names = track.name.to_lowercase().contains(&needle)
            || track
                .artists
                .iter()
                .any(|artist| artist.name.to_lowercase().contains(&needle));
        in_names
            || track
                .album
                .as_ref()
                .is_some_and(|album| album.name.to_lowercase().contains(&needle))
    }
}

impl Library for FakeLibrary {
    fn search(&self, query: &str) -> Result<SearchResults, ApiError> {
        self.gate()?;
        let query = query.trim();
        if query.is_empty() {
            return Ok(SearchResults::default());
        }
        let data = lock(&self.data);
        let needle = query.to_lowercase();
        let named = |name: &str| name.to_lowercase().contains(&needle);
        Ok(SearchResults {
            tracks: top(data
                .tracks
                .iter()
                .filter(|track| Self::matches(query, track))
                .cloned()
                .collect()),
            albums: top(data
                .albums
                .iter()
                .filter(|album| {
                    named(&album.name) || album.artists.iter().any(|artist| named(&artist.name))
                })
                .cloned()
                .collect()),
            artists: top(data
                .artists
                .iter()
                .filter(|artist| named(&artist.name))
                .cloned()
                .collect()),
            playlists: top(data
                .playlists
                .iter()
                .filter(|playlist| named(&playlist.name))
                .cloned()
                .collect()),
        })
    }

    fn playlists(&self, after: Option<&Continuation>) -> Result<Page<Playlist>, ApiError> {
        self.gate()?;
        Ok(self.page(&lock(&self.data).playlists, after))
    }

    fn playlist(&self, id: &str) -> Result<Playlist, ApiError> {
        self.gate()?;
        let data = lock(&self.data);
        let playlist = data
            .playlists
            .iter()
            .find(|playlist| playlist.id == id)
            .cloned()
            .ok_or(ApiError::NotFound)?;
        // `items` is absent for a playlist the user does not own, and that is a
        // field that is missing rather than an empty list.
        Ok(match data.contents.get(id) {
            Some(contents) => Playlist {
                contents: Some(PlaylistContents {
                    total: Some(contents.len() as u32),
                    items: contents.clone(),
                }),
                ..playlist
            },
            None => Playlist {
                contents: None,
                ..playlist
            },
        })
    }

    fn playlist_items(
        &self,
        id: &str,
        after: Option<&Continuation>,
    ) -> Result<Page<TrackItem>, ApiError> {
        self.gate()?;
        let data = lock(&self.data);
        let contents = data.contents.get(id).ok_or(ApiError::NotFound)?;
        Ok(self.page(contents, after))
    }

    fn create_playlist(&self, name: &str, public: bool) -> Result<Playlist, ApiError> {
        self.gate()?;
        self.record("create_playlist");
        let mut data = lock(&self.data);
        data.created += 1;
        let playlist = Playlist {
            id: format!("fake-playlist-{}", data.created),
            name: name.to_string(),
            uri: format!("spotify:playlist:fake-playlist-{}", data.created),
            description: None,
            images: Vec::new(),
            contents: Some(PlaylistContents {
                total: Some(0),
                items: Vec::new(),
            }),
        };
        if public {
            data.playlists.push(playlist.clone());
        }
        Ok(playlist)
    }

    fn add_to_playlist(&self, id: &str, uri: &str) -> Result<(), ApiError> {
        self.gate()?;
        self.record("add_to_playlist");
        let mut data = lock(&self.data);
        let track = data
            .tracks
            .iter()
            .find(|track| track.uri == uri)
            .cloned()
            .ok_or(ApiError::NotFound)?;
        let contents = data.contents.entry(id.to_string()).or_default();
        contents.push(TrackItem { track: Some(track) });
        Ok(())
    }

    fn remove_from_playlist(&self, id: &str, uri: &str) -> Result<(), ApiError> {
        self.gate()?;
        self.record("remove_from_playlist");
        let mut data = lock(&self.data);
        let contents = data.contents.entry(id.to_string()).or_default();
        let before = contents.len();
        contents.retain(|item| item.track.as_ref().is_none_or(|track| track.uri != uri));
        if contents.len() == before {
            return Err(ApiError::NotFound);
        }
        Ok(())
    }

    fn liked_tracks(&self, after: Option<&Continuation>) -> Result<Page<TrackItem>, ApiError> {
        self.gate()?;
        let data = lock(&self.data);
        let items: Vec<TrackItem> = data
            .liked
            .iter()
            .filter_map(|uri| {
                data.tracks
                    .iter()
                    .find(|track| &track.uri == uri)
                    .cloned()
                    .map(|track| TrackItem { track: Some(track) })
            })
            .collect();
        Ok(self.page(&items, after))
    }

    fn is_liked(&self, uri: &str) -> Result<bool, ApiError> {
        self.gate()?;
        Ok(lock(&self.data).liked.iter().any(|liked| liked == uri))
    }

    fn set_liked(&self, uri: &str, liked: bool) -> Result<(), ApiError> {
        self.gate()?;
        self.record("set_liked");
        let mut data = lock(&self.data);
        let held = data.liked.iter().any(|held| held == uri);
        match (liked, held) {
            (true, false) => data.liked.push(uri.to_string()),
            (false, true) => data.liked.retain(|held| held != uri),
            _ => {}
        }
        Ok(())
    }

    fn queue(&self) -> Result<Queue, ApiError> {
        self.gate()?;
        let data = lock(&self.data);
        let (now_playing, upcoming) = match data.queue.split_first() {
            Some((first, rest)) => (Some(first.clone()), rest.to_vec()),
            None => (None, Vec::new()),
        };
        Ok(Queue {
            now_playing,
            upcoming,
        })
    }

    fn enqueue(&self, uri: &str) -> Result<(), ApiError> {
        self.gate()?;
        self.record("enqueue");
        let mut data = lock(&self.data);
        let track = data
            .tracks
            .iter()
            .find(|track| track.uri == uri)
            .cloned()
            .ok_or(ApiError::NotFound)?;
        data.queue.push(track);
        Ok(())
    }

    fn saved_albums(&self, after: Option<&Continuation>) -> Result<Page<Album>, ApiError> {
        self.gate()?;
        Ok(self.page(&lock(&self.data).saved_albums, after))
    }

    fn followed_artists(&self, after: Option<&Continuation>) -> Result<Page<Artist>, ApiError> {
        self.gate()?;
        Ok(self.page(&lock(&self.data).followed, after))
    }

    fn recently_played(&self, after: Option<&Continuation>) -> Result<Page<Track>, ApiError> {
        self.gate()?;
        Ok(self.page(&lock(&self.data).recent, after))
    }

    fn artist_albums(
        &self,
        artist: &str,
        after: Option<&Continuation>,
    ) -> Result<Page<Album>, ApiError> {
        self.gate()?;
        let data = lock(&self.data);
        let albums: Vec<Album> = data
            .albums
            .iter()
            .filter(|album| album.artists.iter().any(|one| one.id == artist))
            .cloned()
            .collect();
        Ok(self.page(&albums, after))
    }

    fn album(&self, id: &str) -> Result<Album, ApiError> {
        self.gate()?;
        lock(&self.data)
            .albums
            .iter()
            .find(|album| album.id == id)
            .cloned()
            .ok_or(ApiError::NotFound)
    }

    fn album_tracks(
        &self,
        id: &str,
        after: Option<&Continuation>,
    ) -> Result<Page<Track>, ApiError> {
        self.gate()?;
        let data = lock(&self.data);
        let tracks: Vec<Track> = data
            .tracks
            .iter()
            .filter(|track| track.album.as_ref().is_some_and(|album| album.id == id))
            .cloned()
            .collect();
        if data.albums.iter().all(|album| album.id != id) {
            return Err(ApiError::NotFound);
        }
        Ok(self.page(&tracks, after))
    }
}

/// The first [`SEARCH_LIMIT`] of a group, which is the whole of a search result.
fn top<T>(found: Vec<T>) -> Vec<T> {
    found.into_iter().take(SEARCH_LIMIT as usize).collect()
}

/// The one row out of a search group that `trak play <name>` plays (TODO 7.12).
///
/// The rule, in shpotify's spirit but written down:
///
/// 1. **The first row whose name contains the whole query, case-insensitively.**
///    A query that names a thing should not lose to a row that merely mentions
///    it in an artist or album field — `play mezzanine` must find the *song*
///    Mezzanine, not whichever track from that album Spotify happened to rank
///    first.
/// 2. **Failing that, the first row outright.** shpotify played the first result
///    of a `limit=1` search and trusted Spotify's relevance ranking, and when no
///    name matches the query there is nothing local to trust instead — the
///    ranking *is* the answer, the way it was for shpotify.
///
/// Deliberately not shpotify's `play list`, which pulled ten results and played
/// one at random: a coin flip is not a match, and this command's whole promise
/// is that it can say which one it picked.
pub fn best_match<'a, T>(rows: &'a [T], query: &str, name: impl Fn(&T) -> &str) -> Option<&'a T> {
    let needle = query.trim().to_lowercase();
    rows.iter()
        .find(|row| name(row).to_lowercase().contains(&needle))
        .or_else(|| rows.first())
}

/// Percent-encode one path segment.
///
/// A Spotify id is base62, so this is a no-op in practice — and it stays here
/// anyway because an id that reaches this module from a response body is not
/// something trak gets to assume. `lyrics.rs` and `web::auth` each have their
/// own private copy for the same reason; this one is for a path segment, which
/// is the third shape.
fn segment(value: &str) -> String {
    percent_encode(value)
}

/// Percent-encode a query or path value.
///
/// Two characters beyond the RFC 3986 unreserved set are left alone, and both
/// are chosen rather than tolerated:
///
/// - `:` because a Spotify URI is the single most common value trak puts in a
///   query (`ids=`, `uris=`, `uri=`) and `spotify%3Atrack%3A…` is unreadable in
///   a status line and in every log that will ever show one.
/// - `,` because it is how `GET /search` lists its types, and the alternative is
///   a search tab that asks for one type per request.
///
/// Everything else outside the unreserved set is escaped, so a title with a
/// space, an `&`, a `#` or a `/` in it cannot change the shape of the URL.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' | b',' => {
                out.push(char::from(*byte));
            }
            other => {
                let _ = write!(out, "%{other:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpListener as StdListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::web::token::{Token, TokenResponse};

    // =======================================================================
    // Fixtures
    //
    // **These are written from the shapes in `docs/WEB-API.md`, not captured
    // from a live call.** There is no Client ID and no login on this machine yet
    // ([owner], TODO 7.2), and nothing in this file may touch the network anyway.
    // What they *are* is faithful in the two ways that matter: every field trak
    // reads is one the doc lists, and every field dev mode removed --
    // `popularity`, `available_markets`, `album_group`, `label`, `followers`,
    // `product` -- is deliberately absent from all of them. The first real login
    // should re-record the ones that matter (TODO 7.7, 7.11) and this note goes
    // with them.
    // =======================================================================

    const ARTIST_ID: &str = "4Z8W4fKeB5YxbusRsdQVPb";
    const ARTIST_NAME: &str = "Massive Attack";
    const ALBUM_ID: &str = "5nMdc39z78kifAc5WXv9Yj";
    const ALBUM_NAME: &str = "Mezzanine";
    const TRACK_ID: &str = "6HacgXCExkzS552ILfJTXu";
    const TRACK_NAME: &str = "Teardrop";
    const TRACK_URI: &str = "spotify:track:6HacgXCExkzS552ILfJTXu";
    const SECOND_TRACK_ID: &str = "5ghIJDpP6863d6KFwdPhJ3";
    const SECOND_TRACK_NAME: &str = "Angel";
    const SECOND_TRACK_URI: &str = "spotify:track:5ghIJDpP6863d6KFwdPhJ3";
    const PLAYLIST_ID: &str = "37i9dQZF1DXcBWIGoYBM5M";
    /// A playlist the user does not own, which Spotify answers with metadata
    /// only.
    const BORROWED_ID: &str = "37i9dQZF1DXcBWIGoYBM6M";
    /// Named so that one query -- `mass` -- finds a row in all four groups, which
    /// is what lets the fake and the real client be compared on a whole
    /// [`SearchResults`] rather than on one group.
    const PLAYLIST_NAME: &str = "Massive Attack on Repeat";

    /// An artist inside a search hit or an album: no images, which is why
    /// [`Artist::images`] is a `Vec` rather than a single field.
    fn artist_stub_json() -> String {
        format!(
            r#"{{"id":"{ARTIST_ID}","name":"{ARTIST_NAME}","uri":"spotify:artist:{ARTIST_ID}"}}"#
        )
    }

    /// An artist with artwork: the shape a full artist object has.
    fn artist_json() -> String {
        format!(
            r#"{{"id":"{ARTIST_ID}","name":"{ARTIST_NAME}","uri":"spotify:artist:{ARTIST_ID}","images":[{{"url":"https://i.scdn.co/image/artist-face","width":640,"height":640}}]}}"#
        )
    }

    /// The album as it appears *inside* a track or in a search hit: no release
    /// date, no track count, no images.
    fn album_stub_json() -> String {
        format!(
            r#"{{"album_type":"album","id":"{ALBUM_ID}","name":"{ALBUM_NAME}","uri":"spotify:album:{ALBUM_ID}","artists":[{artist}]}}"#,
            artist = artist_stub_json()
        )
    }

    /// A full album, from `GET /albums/{id}` and `GET /artists/{id}/albums`. No
    /// `popularity` and no `label`: both are in the dev-mode removal list.
    fn album_json() -> String {
        format!(
            r#"{{"album_type":"album","id":"{ALBUM_ID}","name":"{ALBUM_NAME}","artists":[{artist}],"external_ids":{{"isrc":"GBAYE0601154"}},"images":[{{"url":"https://i.scdn.co/image/album-face","width":640,"height":640}}],"release_date":"1998-04-20","release_date_precision":"day","total_tracks":11,"type":"album","uri":"spotify:album:{ALBUM_ID}"}}"#,
            artist = artist_stub_json()
        )
    }

    /// A track as a search returns it. No `popularity`.
    fn teardrop_json() -> String {
        format!(
            r#"{{"album":{album},"artists":[{artist}],"disc_number":1,"duration_ms":301000,"explicit":false,"external_ids":{{"isrc":"GBAYE0601498"}},"id":"{TRACK_ID}","is_local":false,"name":"{TRACK_NAME}","track_number":10,"type":"track","uri":"{TRACK_URI}"}}"#,
            album = album_stub_json(),
            artist = artist_stub_json()
        )
    }

    /// A track as a tracklist returns it: `external_ids` and `is_local` gone,
    /// `explicit` present. A parser pinned to one of the two shapes proves
    /// nothing about the other.
    fn angel_json() -> String {
        format!(
            r#"{{"album":{album},"artists":[{artist}],"disc_number":1,"duration_ms":380000,"explicit":true,"id":"{SECOND_TRACK_ID}","name":"{SECOND_TRACK_NAME}","track_number":1,"type":"track","uri":"{SECOND_TRACK_URI}"}}"#,
            album = album_stub_json(),
            artist = artist_stub_json()
        )
    }

    /// A podcast episode in a playlist. It really is shaped like this, and it
    /// really does carry only the fields trak shows, so it reads as a track with
    /// no artists and no album.
    fn episode_json() -> String {
        r#"{"id":"512ojhOuo1ktJprKbVcKyQ","name":"An episode","release_date":"2026-01-31","duration_ms":1800000,"explicit":false,"uri":"spotify:episode:512ojhOuo1ktJprKbVcKyQ"}"#
        .to_string()
    }

    /// A playlist in a search hit and in `/me/playlists`: `items` is a count,
    /// with no array in it. That is the renamed `tracks.total`.
    /// One playlist as `GET /me/playlists` returns it.
    ///
    /// The `description` and `images` are here because Spotify sends them in the
    /// list, not only in the detail. A fixture that omits them makes the fake and
    /// the real client look like they disagree about the same playlist when what
    /// actually happened is that the fixture described two different playlists.
    fn playlist_stub_json() -> String {
        format!(
            r#"{{"id":"{PLAYLIST_ID}","name":"{PLAYLIST_NAME}","uri":"spotify:playlist:{PLAYLIST_ID}","type":"playlist","description":"Long drives","images":[{{"url":"https://i.scdn.co/image/playlist-face","width":300,"height":300}}],"items":{{"total":3}}}}"#
        )
    }

    /// The three rows of the owned playlist, in the documented nesting:
    /// `items.items.item`.
    fn playlist_rows() -> Vec<String> {
        vec![
            format!(
                r#"{{"added_at":"2026-02-02T11:00:00Z","track":{track}}}"#,
                track = teardrop_json()
            ),
            format!(
                r#"{{"added_at":"2026-02-02T11:01:00Z","track":{track}}}"#,
                track = angel_json()
            ),
            format!(
                r#"{{"added_at":"2026-02-03T09:00:00Z","track":{track}}}"#,
                track = episode_json()
            ),
        ]
    }

    /// `GET /playlists/{id}` for a playlist the user owns or collaborates on.
    /// What `POST /me/playlists` answers with: the identity, and an item count of
    /// zero.
    ///
    /// Spotify does not put anything in a playlist the moment it is created, and
    /// a fixture that replies with three rows makes the create path look like it
    /// invents a catalogue. The `items` field is present but empty, which is the
    /// shape an owned playlist has.
    fn playlist_body_created() -> String {
        playlist_body(
            PLAYLIST_ID,
            Some(r#"{"total":0,"limit":100,"offset":0,"next":null,"items":[]}"#.to_string()),
            true,
        )
    }

    fn playlist_body_owned(next: Option<&str>) -> String {
        let next = next.map_or_else(|| "null".to_string(), |url| format!("\"{url}\""));
        let rows = playlist_rows().join(",");
        playlist_body(
            PLAYLIST_ID,
            Some(format!(
                r#"{{"total":3,"limit":100,"offset":0,"next":{next},"items":[{rows}]}}"#
            )),
            true,
        )
    }

    /// `GET /playlists/{id}` for a playlist trak may not read: "only metadata is
    /// returned and the `items` field will be absent".
    fn playlist_body_metadata_only() -> String {
        playlist_body(BORROWED_ID, None, false)
    }

    fn playlist_body(id: &str, items: Option<String>, with_description: bool) -> String {
        let description = with_description.then_some(r#""description":"Long drives","#);
        let items = items.map_or(String::new(), |items| format!(r#","items":{items}"#));
        format!(
            r#"{{"collaborative":false,{description}"external_urls":{{"spotify":"https://open.spotify.com/playlist/{id}"}},"followers":{{"href":null,"total":1}},"id":"{id}","images":[{{"url":"https://i.scdn.co/image/playlist-face","width":300,"height":300}}],"name":"{PLAYLIST_NAME}","owner":{{"display_name":"someone else","id":"someone"}},"public":true,"snapshot_id":"snapshot","uri":"spotify:playlist:{id}"{items}}}"#,
            description = description.unwrap_or_default(),
            items = items,
        )
    }

    /// `GET /search`, with one row in each of the four groups. The query that
    /// finds all of them is `mass`.
    fn search_body() -> String {
        format!(
            r#"{{"tracks":{{"href":"https://api.spotify.com/v1/search","items":[{teardrop},{angel}],"limit":10,"next":null,"total":2}},"albums":{{"href":"x","items":[{album}],"limit":10,"next":null,"total":1}},"artists":{{"href":"x","items":[{artist}],"limit":10,"next":null,"total":1}},"playlists":{{"href":"x","items":[{playlist}],"limit":10,"next":null,"total":1}}}}"#,
            teardrop = teardrop_json(),
            angel = angel_json(),
            album = album_json(),
            artist = artist_json(),
            playlist = playlist_stub_json(),
        )
    }

    /// A paged body. `next` is an absolute URL, which is the only shape a
    /// continuation may take -- see `SpotifyLibrary::continuation_url`.
    fn paged(rows: &[String], next: Option<&str>) -> String {
        format!(
            r#"{{"href":"https://api.spotify.com/v1/x","items":[{}],"limit":100,"next":{},"offset":0,"previous":null,"total":{}}}"#,
            rows.join(","),
            next.map_or_else(|| "null".to_string(), |url| format!("\"{url}\"")),
            rows.len(),
        )
    }

    /// The `{"track": …}` wrapper `GET /me/tracks` and
    /// `GET /playlists/{id}/items` both use.
    fn wrapped(tracks: &[String]) -> Vec<String> {
        tracks
            .iter()
            .map(|track| {
                format!(
                    r#"{{"added_at":"2026-03-01T08:00:00Z","track":{track}}}"#,
                    track = track
                )
            })
            .collect()
    }

    fn liked_body(next: Option<&str>) -> String {
        paged(&wrapped(&[teardrop_json(), angel_json()]), next)
    }

    fn playlist_items_body(next: Option<&str>) -> String {
        paged(&playlist_rows(), next)
    }

    fn recently_body(next: Option<&str>) -> String {
        paged(
            &[
                format!(
                    r#"{{"track":{track},"played_at":"2026-09-29T20:00:00.000Z"}}"#,
                    track = teardrop_json()
                ),
                format!(
                    r#"{{"track":{track},"played_at":"2026-09-29T19:00:00.000Z"}}"#,
                    track = angel_json()
                ),
            ],
            next,
        )
    }

    fn album_tracks_body(next: Option<&str>) -> String {
        paged(&wrapped(&[teardrop_json(), angel_json()]), next)
    }

    /// One album, as small as the model allows, for the cache tests.
    fn album_body(id: &str) -> String {
        format!(r#"{{"id":"{id}","name":"Album {id}","uri":"spotify:album:{id}"}}"#)
    }

    // --- the same data, written out by hand -------------------------------
    //
    // Deliberately *not* parsed from the fixtures above: a fake seeded by the
    // parser under test would agree with it whatever both got wrong.

    fn image(url: &str, side: u32) -> Image {
        Image {
            url: url.to_string(),
            width: Some(side),
            height: Some(side),
        }
    }

    fn artist() -> Artist {
        Artist {
            id: ARTIST_ID.to_string(),
            name: ARTIST_NAME.to_string(),
            uri: format!("spotify:artist:{ARTIST_ID}"),
            images: vec![image("https://i.scdn.co/image/artist-face", 640)],
        }
    }

    fn artist_stub() -> Artist {
        Artist {
            images: Vec::new(),
            ..artist()
        }
    }

    fn album() -> Album {
        Album {
            id: ALBUM_ID.to_string(),
            name: ALBUM_NAME.to_string(),
            uri: format!("spotify:album:{ALBUM_ID}"),
            release_date: Some("1998-04-20".to_string()),
            artists: vec![artist_stub()],
            images: vec![image("https://i.scdn.co/image/album-face", 640)],
            total_tracks: Some(11),
        }
    }

    fn album_stub() -> Album {
        Album {
            release_date: None,
            images: Vec::new(),
            total_tracks: None,
            ..album()
        }
    }

    fn teardrop() -> Track {
        Track {
            id: TRACK_ID.to_string(),
            name: TRACK_NAME.to_string(),
            uri: TRACK_URI.to_string(),
            duration_ms: 301_000,
            track_number: Some(10),
            disc_number: Some(1),
            artists: vec![artist_stub()],
            album: Some(album_stub()),
        }
    }

    fn angel() -> Track {
        Track {
            id: SECOND_TRACK_ID.to_string(),
            name: SECOND_TRACK_NAME.to_string(),
            uri: SECOND_TRACK_URI.to_string(),
            duration_ms: 380_000,
            track_number: Some(1),
            disc_number: Some(1),
            artists: vec![artist_stub()],
            album: Some(album_stub()),
        }
    }

    fn episode() -> Track {
        Track {
            id: "512ojhOuo1ktJprKbVcKyQ".to_string(),
            name: "An episode".to_string(),
            uri: "spotify:episode:512ojhOuo1ktJprKbVcKyQ".to_string(),
            duration_ms: 1_800_000,
            track_number: None,
            disc_number: None,
            artists: Vec::new(),
            album: None,
        }
    }

    /// A playlist in a list response: metadata and a count, no array.
    /// A playlist as the *list* endpoints return it: the identity, the
    /// description and the image, plus the item count with no items in it.
    fn playlist_stub() -> Playlist {
        Playlist {
            id: PLAYLIST_ID.to_string(),
            name: PLAYLIST_NAME.to_string(),
            uri: format!("spotify:playlist:{PLAYLIST_ID}"),
            description: Some("Long drives".to_string()),
            images: vec![image("https://i.scdn.co/image/playlist-face", 300)],
            contents: Some(PlaylistContents {
                total: Some(3),
                items: Vec::new(),
            }),
        }
    }

    /// A playlist read in full, for one the user owns.
    fn playlist_owned() -> Playlist {
        Playlist {
            contents: Some(PlaylistContents {
                total: Some(3),
                items: vec![
                    TrackItem {
                        track: Some(teardrop()),
                    },
                    TrackItem {
                        track: Some(angel()),
                    },
                    TrackItem {
                        track: Some(episode()),
                    },
                ],
            }),
            ..playlist_stub()
        }
    }

    /// A token to hand the client. Nothing here ever sends it anywhere real.
    fn token() -> Token {
        token_with("access-abc")
    }

    fn token_with(access: &str) -> Token {
        Token::from_response(
            &TokenResponse {
                access_token: access.to_string(),
                refresh_token: Some("refresh-xyz".to_string()),
                expires_in: Some(3600),
                scope: Some("user-library-read".to_string()),
            },
            UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        )
        .expect("a token from a good response")
    }

    fn system_now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_800_000_000)
    }

    // =======================================================================
    // The guard: what trak is allowed to ask for
    // =======================================================================

    /// The query a paged endpoint may carry. **`offset` and `limit` are here only
    /// because they arrive inside the `next` URL Spotify sends**, and
    /// `trak_never_invents_a_paging_parameter` is the test that proves trak never
    /// adds one itself.
    const PAGED: &[&str] = &["offset", "limit"];

    /// Every endpoint this build may call, and the query parameters each may
    /// carry. `None` means the trinity is not on the list -- which is the state
    /// every removed endpoint in `docs/WEB-API.md` §3 is in.
    fn allowed(method: &str, path: &str) -> Option<&'static [&'static str]> {
        match (method, path) {
            ("GET", "/search") => Some(&["q", "type", "limit"]),
            ("GET", "/me/playlists") => Some(PAGED),
            ("GET", "/me/tracks") => Some(PAGED),
            ("GET", "/me/albums") => Some(PAGED),
            ("GET", "/me/library/contains") => Some(&["ids", "types"]),
            ("POST", "/me/playlists") => Some(&[]),
            ("GET", "/me/player/queue") => Some(&[]),
            ("POST", "/me/player/queue") => Some(&["uri"]),
            ("GET", "/me/player/recently-played") => Some(PAGED),
            ("GET", "/me/following") => Some(&["type", "offset", "limit"]),
            ("PUT" | "DELETE", "/me/library") => Some(&[]),
            _ => one_id(method, path),
        }
    }

    /// The endpoints that carry a single id in the path: every per-resource read
    /// and every playlist write.
    fn one_id(method: &str, path: &str) -> Option<&'static [&'static str]> {
        let segments: Vec<&str> = path.split('/').collect();
        if segments.len() < 3 || segments[2].is_empty() {
            return None;
        }
        match (method, segments.as_slice()) {
            ("GET", ["", "playlists", _]) => Some(&[]),
            ("GET", ["", "playlists", _, "items"]) => Some(PAGED),
            ("POST", ["", "playlists", _, "items"]) => Some(&["uris"]),
            ("DELETE", ["", "playlists", _, "items"]) => Some(&[]),
            ("GET", ["", "albums", _]) => Some(&[]),
            ("GET", ["", "albums", _, "tracks"]) => Some(PAGED),
            ("GET", ["", "artists", _, "albums"]) => Some(PAGED),
            _ => None,
        }
    }

    /// Whether a trinity survives the guard, and why not if it does not.
    ///
    /// The value of this one function is that it is *the* rule: the mock below
    /// applies it to every request in this file, so a future method that reaches
    /// for a removed endpoint fails whichever test happens to call it.
    fn refusal(method: &str, path: &str, query: &[(String, String)]) -> Option<String> {
        let Some(keys) = allowed(method, path) else {
            return Some(format!(
                "{method} {path} is not an endpoint docs/WEB-API.md leaves available"
            ));
        };
        for (name, value) in query {
            if !keys.contains(&name.as_str()) {
                return Some(format!("{path} does not take a {name} parameter"));
            }
            // The only cap the doc publishes anywhere is search's ten. Everywhere
            // else trak sends no limit at all, and the one it does send is a
            // ceiling rather than a preference.
            if name == "limit" && path == "/search" {
                match value.parse::<u32>() {
                    Ok(limit) if limit <= SEARCH_LIMIT => {}
                    _ => {
                        return Some(format!(
                            "search asked for {value}, above the documented {SEARCH_LIMIT}"
                        ));
                    }
                }
            }
            // An endpoint that takes a list must be handed one id. Two ids is a
            // batch request wearing a legal endpoint's clothes, and that is
            // exactly what the removal of the batch endpoints forbids.
            if matches!(name.as_str(), "ids" | "uris") && value.split(',').count() != 1 {
                return Some(format!(
                    "{path} was handed {} ids",
                    value.split(',').count()
                ));
            }
        }
        None
    }

    /// Every endpoint `docs/WEB-API.md` §3 records as removed in dev mode, with a
    /// trinity that would reach it. The batch list is the important one: those
    /// are the calls `rspotify`'s id-list helpers used to make.
    #[test]
    fn guard_covers_every_removed_endpoint() {
        let removed = [
            // Every multi-item fetch. One id per request is the rule.
            ("GET", "/tracks"),
            ("GET", "/albums"),
            ("GET", "/artists"),
            ("GET", "/episodes"),
            ("GET", "/shows"),
            ("GET", "/audiobooks"),
            ("GET", "/chapters"),
            // Browsing, markets and the per-user endpoints.
            ("GET", "/markets"),
            ("GET", "/browse/new-releases"),
            ("GET", "/browse/categories"),
            ("GET", "/browse/categories/party-music"),
            ("GET", "/users/kathir"),
            ("GET", "/users/kathir/playlists"),
            ("POST", "/users/kathir/playlists"),
            // The artist top tracks, with no replacement offered.
            ("GET", "/artists/4Z8W4fKeB5YxbusRsdQVPb/top-tracks"),
            // The library writes that became `/me/library`, and the like check
            // that became `/me/library/contains`.
            ("PUT", "/me/tracks"),
            ("DELETE", "/me/tracks"),
            ("GET", "/me/tracks/contains"),
            ("PUT", "/me/following"),
            ("DELETE", "/me/following"),
            // Playlist items moved from `/tracks` to `/items`.
            ("GET", "/playlists/37i9dQZF1DXcBWIGoYBM5M/tracks"),
            ("POST", "/playlists/37i9dQZF1DXcBWIGoYBM5M/tracks"),
            ("DELETE", "/playlists/37i9dQZF1DXcBWIGoYBM5M/tracks"),
            // Top items need a scope no trak tab has, and `/me` lost the fields a
            // tab would have wanted.
            ("GET", "/me/top/tracks"),
        ];
        for (method, path) in removed {
            assert!(
                allowed(method, path).is_none(),
                "{method} {path} was removed in dev mode and must not be callable"
            );
            assert!(
                refusal(method, path, &[]).is_some(),
                "{method} {path} should be refused with a reason"
            );
        }
        // The endpoints that *did* survive are still callable, so the guard is a
        // list rather than a refusal of everything.
        assert!(
            allowed("GET", "/me/tracks").is_some(),
            "the liked-songs read stays"
        );
        assert!(allowed("GET", "/me/albums").is_some(), "saved albums stays");
    }

    #[test]
    fn the_guard_refuses_a_limit_above_the_documented_maximum() {
        assert!(
            refusal("GET", "/search", &[("limit".into(), "50".into())]).is_some(),
            "50 was the pre-dev-mode maximum"
        );
        assert_eq!(
            refusal("GET", "/search", &[("limit".into(), "10".into())]),
            None,
            "10 is what the doc allows"
        );
    }

    /// Two ids down a surviving endpoint. Nothing about the endpoint says batch.
    #[test]
    fn the_guard_refuses_more_than_one_id_per_request() {
        let two = vec![("ids".to_string(), format!("{TRACK_URI},{TRACK_URI}"))];
        assert!(refusal("GET", "/me/library/contains", &two).is_some());
        let one = vec![("ids".to_string(), TRACK_URI.to_string())];
        assert_eq!(refusal("GET", "/me/library/contains", &one), None);
    }

    #[test]
    fn the_guard_refuses_a_parameter_the_endpoint_does_not_take() {
        // `market` and `offset` are real Spotify parameters; trak sends neither,
        // because a page size is the server's decision and a market is a setting
        // this task does not have.
        for query in [
            vec![("market".to_string(), "GB".to_string())],
            vec![("offset".to_string(), "20".to_string())],
            vec![("limit".to_string(), "10".to_string())],
        ] {
            assert!(
                refusal("GET", "/me/player/queue", &query).is_some(),
                "{query:?} should be refused"
            );
        }
    }

    // =======================================================================
    // The mock server
    // =======================================================================

    /// A request as its three parts plus what came with it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Recorded {
        method: String,
        path: String,
        query: Vec<(String, String)>,
        body: String,
        authorization: Option<String>,
    }

    impl Recorded {
        /// The pair the guard matches on.
        fn endpoint(&self) -> (&str, &str) {
            (&self.method, self.path.as_str())
        }

        fn param(&self, name: &str) -> Option<&str> {
            self.query
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.as_str())
        }
    }

    /// What a route answers with.
    #[derive(Clone)]
    enum Spec {
        Answer {
            status: u16,
            headers: Vec<(String, String)>,
            body: String,
        },
        /// Accept the request and then say nothing, which is what a hung API
        /// looks like from here.
        Stall,
        /// Close the connection without a response at all.
        Cut,
    }

    /// One canned answer, matching on method and path and used once unless
    /// `times` says otherwise. `"*"` as the path matches any path.
    struct Route {
        method: String,
        path: String,
        times: usize,
        spec: Spec,
    }

    impl Route {
        fn new(method: &str, path: &str) -> Self {
            Self {
                method: method.to_string(),
                path: path.to_string(),
                times: 1,
                spec: Spec::Answer {
                    status: 200,
                    headers: Vec::new(),
                    body: String::new(),
                },
            }
        }

        /// A route for any path, for a test that is about the status and not
        /// about the endpoint.
        fn anywhere(method: &str) -> Self {
            Self::new(method, "*")
        }

        /// Set the status and the body, keeping any header already added -- so
        /// `Route::new(..).header(..).reply(..)` reads in that order and works.
        fn reply(mut self, status: u16, body: &str) -> Self {
            if let Spec::Answer {
                status: current,
                body: text,
                ..
            } = &mut self.spec
            {
                *current = status;
                *text = body.to_string();
            }
            self
        }

        fn header(mut self, name: &str, value: &str) -> Self {
            if let Spec::Answer { headers, .. } = &mut self.spec {
                headers.push((name.to_string(), value.to_string()));
            }
            self
        }

        /// Serve this route `n` times instead of once, for a request trak makes
        /// more than once on purpose.
        fn times(mut self, n: usize) -> Self {
            self.times = n;
            self
        }

        fn stalling() -> Self {
            Self {
                spec: Spec::Stall,
                ..Self::anywhere("GET")
            }
        }

        fn cutting() -> Self {
            Self {
                spec: Spec::Cut,
                ..Self::anywhere("GET")
            }
        }
    }

    /// A loopback HTTP server that answers from a list of canned routes and
    /// records what it was sent. Real sockets and real HTTP, so the URL building,
    /// the verbs, the bodies and the status handling are all covered, with no
    /// network.
    struct Mock {
        base: String,
        routes: Arc<Mutex<Vec<Route>>>,
        requests: Arc<Mutex<Vec<Recorded>>>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    /// The path prefix this mock is rooted at, which is what makes a request for
    /// `/search` arrive as `/v1/search`.
    const PREFIX: &str = "/v1";

    impl Mock {
        fn new() -> Self {
            let listener = StdListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("mock binds");
            listener.set_nonblocking(true).expect("mock nonblocking");
            let port = listener.local_addr().expect("mock addr").port();
            let routes: Arc<Mutex<Vec<Route>>> = Arc::new(Mutex::new(Vec::new()));
            let requests: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let thread = {
                let routes = Arc::clone(&routes);
                let requests = Arc::clone(&requests);
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        match listener.accept() {
                            Ok((mut stream, _)) => {
                                // The trap the production loopback has to avoid:
                                // a socket accepted from a non-blocking listener is
                                // non-blocking on macOS, so a request that has not
                                // arrived yet reads as "no request".
                                let _ = stream.set_nonblocking(false);
                                let Some(raw) = read_request(&mut stream) else {
                                    continue;
                                };
                                let recorded = parse_target(&raw);
                                // The guard runs in the *server*, which is what
                                // makes every test in this file a guard test too.
                                if let Some(why) = refusal(
                                    recorded.method.as_str(),
                                    recorded.path.as_str(),
                                    &recorded.query,
                                ) {
                                    let body = format!(
                                        r#"{{"error":{{"status":400,"message":"trak asked for something it may not: {why}"}}}}"#
                                    );
                                    let _ =
                                        stream.write_all(http_response(400, &[], &body).as_bytes());
                                    let _ = stream.flush();
                                    continue;
                                }
                                let spec = take_route(&routes, &recorded);
                                requests.lock().expect("mock lock").push(recorded);
                                match spec {
                                    Spec::Answer {
                                        status,
                                        headers,
                                        body,
                                    } => {
                                        let response = http_response(status, &headers, &body);
                                        let _ = stream.write_all(response.as_bytes());
                                        let _ = stream.flush();
                                    }
                                    Spec::Stall => {
                                        // Long enough for any timeout a test sets,
                                        // short enough that dropping the mock does
                                        // not hold the suite up.
                                        thread::sleep(Duration::from_millis(400));
                                    }
                                    Spec::Cut => {
                                        let _ = stream.shutdown(std::net::Shutdown::Both);
                                    }
                                }
                            }
                            // An accept error is this mock's problem, not the
                            // test's: dying here would turn into a slow "could
                            // not be reached" in whichever test ran next.
                            Err(_) => thread::sleep(Duration::from_millis(2)),
                        }
                    }
                })
            };
            Self {
                base: format!("http://{}:{port}{PREFIX}", Ipv4Addr::LOCALHOST),
                routes,
                requests,
                stop,
                thread: Some(thread),
            }
        }

        /// Add a canned answer.
        fn add(&self, route: Route) {
            self.routes.lock().expect("mock lock").push(route);
        }

        fn client(&self) -> SpotifyLibrary {
            SpotifyLibrary::at(&self.base, &token())
        }

        fn requests(&self) -> Vec<Recorded> {
            self.requests.lock().expect("mock lock").clone()
        }

        fn paths(&self) -> Vec<(String, String)> {
            self.requests()
                .iter()
                .map(|request| (request.method.clone(), request.path.clone()))
                .collect()
        }
    }

    impl Drop for Mock {
        fn drop(&mut self) {
            // Unblock the accept loop, so no test leaves a thread parked on a
            // socket for the rest of the run.
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// Pull the first route that matches, dropping it once it has been used.
    fn take_route(routes: &Mutex<Vec<Route>>, request: &Recorded) -> Spec {
        let mut routes = routes.lock().expect("mock lock");
        let Some(at) = routes.iter().position(|route| {
            (route.path == "*" || route.path == request.path)
                && (route.method == "*" || route.method == request.method)
        }) else {
            // A request nothing was prepared for is a broken test, and saying so
            // in the body makes it fail on the client's error rather than hang.
            return Spec::Answer {
                status: 500,
                headers: Vec::new(),
                body: format!(
                    r#"{{"error":{{"status":500,"message":"no route for {} {}"}}}}"#,
                    request.method, request.path
                ),
            };
        };
        if routes[at].times > 1 {
            routes[at].times -= 1;
            routes[at].spec.clone()
        } else {
            routes.remove(at).spec
        }
    }

    /// One request as text: headers plus exactly the body the `Content-Length`
    /// promises. A body cut short is a JSON document that does not parse, which
    /// reads as a product bug rather than as a broken mock.
    fn read_request(stream: &mut TcpStream) -> Option<String> {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let mut request = Vec::new();
        let mut chunk = [0u8; 1024];
        let mut end = None;
        while end.is_none() {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    request.extend_from_slice(&chunk[..n]);
                    end = request
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .map(|at| at + 4);
                }
                Err(_) => break,
            }
        }
        let end = end?;
        let length = content_length(&request[..end]);
        while request.len() < end + length {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => request.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
        Some(String::from_utf8_lossy(&request).into_owned())
    }

    fn parse_target(raw: &str) -> Recorded {
        let head = raw.split("\r\n\r\n").next().unwrap_or_default();
        let mut lines = head.lines();
        let mut request_line = lines.next().unwrap_or_default().split_whitespace();
        let method = request_line.next().unwrap_or_default().to_string();
        let target = origin_form(request_line.next().unwrap_or_default());
        let (raw_path, raw_query) = match target.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (target, None),
        };
        Recorded {
            method,
            path: raw_path
                .strip_prefix(PREFIX)
                .unwrap_or(raw_path)
                .to_string(),
            query: raw_query.map(query_params).unwrap_or_default(),
            body: raw
                .split_once("\r\n\r\n")
                .map_or("", |(_, body)| body)
                .to_string(),
            authorization: lines
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                .map(|(_, value)| value.trim().to_string()),
        }
    }

    /// The path out of a request target.
    ///
    /// ureq sends the absolute form -- `GET http://host/v1/search HTTP/1.1` --
    /// for this client, so the scheme and authority are stripped rather than
    /// assumed away. A server that answered origin-form only would be a different
    /// client, and one that has to be made to work with a special case is one
    /// whose case is a fact about the request.
    fn origin_form(target: &str) -> &str {
        let Some(after_scheme) = target.split_once("://").map(|(_, rest)| rest) else {
            return target;
        };
        match after_scheme.find('/') {
            Some(at) => &after_scheme[at..],
            // `GET http://host HTTP/1.1` is a request for the root.
            None => "/",
        }
    }

    fn query_params(query: &str) -> Vec<(String, String)> {
        query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| match pair.split_once('=') {
                Some((key, value)) => (key.to_string(), value.to_string()),
                None => (pair.to_string(), String::new()),
            })
            .collect()
    }

    fn content_length(headers: &[u8]) -> usize {
        String::from_utf8_lossy(headers)
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse().ok())
            .unwrap_or(0)
    }

    fn http_response(status: u16, headers: &[(String, String)], body: &str) -> String {
        let reason = match status {
            200 => "OK",
            201 => "Created",
            204 => "No Content",
            400 => "Bad Request",
            401 => "Unauthorized",
            403 => "Forbidden",
            404 => "Not Found",
            429 => "Too Many Requests",
            500 => "Internal Server Error",
            _ => "Error",
        };
        let mut response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        for (name, value) in headers {
            response.push_str(&format!("{name}: {value}\r\n"));
        }
        response.push_str("\r\n");
        response.push_str(body);
        response
    }

    // =======================================================================
    // Happy paths
    // =======================================================================

    #[test]
    fn search_returns_the_four_groups_from_a_recorded_response() {
        let mock = Mock::new();
        mock.add(Route::new("GET", "/search").reply(200, &search_body()));
        let client = mock.client();

        let results = client.search("mass").expect("search");

        assert_eq!(results.tracks, vec![teardrop(), angel()]);
        assert_eq!(results.albums, vec![album()]);
        assert_eq!(results.artists, vec![artist()]);
        assert_eq!(results.playlists, vec![playlist_stub()]);
        assert!(!results.is_empty());
    }

    /// The whole of the search request, pinned: `limit` at the documented
    /// maximum, all four types, and nothing trak has no use for.
    #[test]
    fn search_asks_for_ten_per_group_and_nothing_else() {
        let mock = Mock::new();
        mock.add(Route::new("GET", "/search").reply(200, &search_body()));
        let client = mock.client();
        client.search("  mass  ").expect("search");

        let request = &mock.requests()[0];
        assert_eq!(request.endpoint(), ("GET", "/search"));
        assert_eq!(request.param("q"), Some("mass"), "and the query is trimmed");
        assert_eq!(
            request.param("type"),
            Some(SEARCH_TYPES),
            "commas travel as commas"
        );
        assert_eq!(request.param("limit"), Some("10"));
        assert_eq!(
            request.query.len(),
            3,
            "no market, no offset, no include_external: {request:?}"
        );
    }

    /// A debounce can fire before a single character is typed, and an empty `q`
    /// is a 400 the user did not ask for.
    #[test]
    fn an_empty_query_spends_no_request() {
        let mock = Mock::new();
        let client = mock.client();
        for query in ["", "   ", "\t"] {
            assert!(
                client.search(query).expect("search").is_empty(),
                "{query:?}"
            );
        }
        assert!(
            mock.requests().is_empty(),
            "no request should have been made"
        );
    }

    #[test]
    fn search_can_return_nothing_at_all() {
        let mock = Mock::new();
        mock.add(Route::new("GET", "/search").reply(
            200,
            r#"{"tracks":{"items":[],"next":null},"albums":{"items":[],"next":null},"artists":{"items":[],"next":null},"playlists":{"items":[],"next":null}}"#,
        ));
        assert!(mock.client().search("qqq").expect("search").is_empty());
    }

    /// A 200 with one group missing is an empty group, not a failed search: the
    /// other three are still worth showing.
    #[test]
    fn a_group_missing_from_the_response_is_an_empty_group() {
        let mock = Mock::new();
        mock.add(Route::new("GET", "/search").reply(
            200,
            &format!(
                r#"{{"tracks":{{"items":[{teardrop}]}},"albums":{{"items":[]}}}}"#,
                teardrop = teardrop_json()
            ),
        ));
        let results = mock.client().search("mass").expect("search");
        assert_eq!(results.tracks, vec![teardrop()]);
        assert!(results.albums.is_empty());
        assert!(results.artists.is_empty());
        assert!(results.playlists.is_empty());
    }

    #[test]
    fn playlists_are_paged_and_the_next_page_comes_from_the_body() {
        let mock = Mock::new();
        let second = format!("{}/me/playlists?offset=100&limit=100", mock.base);
        mock.add(
            Route::new("GET", "/me/playlists")
                .reply(200, &paged(&[playlist_stub_json()], Some(&second))),
        );
        mock.add(Route::new("GET", "/me/playlists").reply(
            200,
            &paged(
                &[r#"{"id":"37i9dQZF1DXcBWIGoYBM5N","name":"Late","uri":"spotify:playlist:37i9dQZF1DXcBWIGoYBM5N"}"#.to_string()],
                None,
            ),
        ));
        let client = mock.client();

        let first = client.playlists(None).expect("page one");
        assert_eq!(first.items, vec![playlist_stub()]);
        let next = first.next.expect("a next page");
        let last = client.playlists(Some(&next)).expect("page two");
        assert_eq!(last.items[0].name, "Late");
        assert!(last.next.is_none(), "the end of the list");
        assert!(!last.is_empty(), "a page with rows is not empty");

        // The first page asked with *no* query string at all, and the second went
        // exactly where the body said, offset and all.
        assert_eq!(mock.requests()[0].query, Vec::new());
        assert_eq!(
            mock.requests()[1].query,
            vec![
                ("offset".to_string(), "100".to_string()),
                ("limit".to_string(), "100".to_string()),
            ]
        );
        assert_eq!(
            mock.paths(),
            vec![
                ("GET".to_string(), "/me/playlists".to_string()),
                ("GET".to_string(), "/me/playlists".to_string()),
            ]
        );
    }

    /// The first page of every paged endpoint is asked for with no query string,
    /// because `docs/WEB-API.md` publishes a `limit` for search and for nothing
    /// else. A paging parameter trak invented would be a documented limit ignored.
    #[test]
    fn trak_never_invents_a_paging_parameter() {
        let mock = Mock::new();
        for (method, path) in [
            ("GET", "/me/playlists"),
            ("GET", "/me/tracks"),
            ("GET", "/me/albums"),
            ("GET", "/me/following"),
            ("GET", "/me/player/recently-played"),
            ("GET", "/playlists/37i9dQZF1DXcBWIGoYBM5M/items"),
            ("GET", "/albums/5nMdc39z78kifAc5WXv9Yj/tracks"),
            ("GET", "/artists/4Z8W4fKeB5YxbusRsdQVPb/albums"),
        ] {
            mock.add(Route::new(method, path).reply(200, r#"{"items":[]}"#));
        }
        let client = mock.client();
        client.playlists(None).expect("playlists");
        client.liked_tracks(None).expect("liked");
        client.saved_albums(None).expect("albums");
        client.followed_artists(None).expect("following");
        client.recently_played(None).expect("recent");
        client
            .playlist_items(PLAYLIST_ID, None)
            .expect("playlist items");
        client.album_tracks(ALBUM_ID, None).expect("album tracks");
        client
            .artist_albums(ARTIST_ID, None)
            .expect("artist albums");

        /// Method, path, and the query it carried.
        type Carrying = (String, String, Vec<(String, String)>);
        let carrying: Vec<Carrying> = mock
            .requests()
            .iter()
            .filter(|request| !request.query.is_empty())
            .map(|request| {
                (
                    request.method.clone(),
                    request.path.clone(),
                    request.query.clone(),
                )
            })
            .collect();
        assert_eq!(
            carrying,
            vec![(
                "GET".to_string(),
                "/me/following".to_string(),
                vec![("type".to_string(), "artist".to_string())],
            )],
            "only /me/following takes a parameter trak supplies, and that one is \
             the endpoint's own required `type`"
        );
    }

    /// A `next` pointing somewhere else is refused rather than requested with the
    /// user's bearer token attached.
    #[test]
    fn a_continuation_from_another_host_is_refused() {
        let mock = Mock::new();
        mock.add(Route::new("GET", "/me/playlists").reply(
            200,
            &paged(
                &[],
                Some("https://example.invalid/v1/me/playlists?offset=100"),
            ),
        ));
        let client = mock.client();
        let first = client.playlists(None).expect("page one");
        let next = first.next.expect("a next page");

        assert_eq!(
            client.playlists(Some(&next)),
            Err(ApiError::Malformed),
            "a next that is not this API is not followed"
        );
        assert_eq!(
            mock.requests().len(),
            1,
            "and nothing was sent to the other host"
        );
    }

    #[test]
    fn playlist_detail_carries_items_only_when_spotify_returns_them() {
        let mock = Mock::new();
        mock.add(
            Route::new("GET", &format!("/playlists/{PLAYLIST_ID}"))
                .reply(200, &playlist_body_owned(None)),
        );
        mock.add(
            Route::new("GET", &format!("/playlists/{BORROWED_ID}"))
                .reply(200, &playlist_body_metadata_only()),
        );
        let client = mock.client();

        let owned = client.playlist(PLAYLIST_ID).expect("playlist");
        assert_eq!(owned, playlist_owned());
        let contents = owned.contents.expect("items for a playlist the user owns");
        assert_eq!(contents.total, Some(3));
        assert_eq!(contents.items.len(), 3);

        // The documented other half: for a playlist the user does not own,
        // "only metadata is returned and the `items` field will be absent".
        let borrowed = client.playlist(BORROWED_ID).expect("playlist");
        assert!(borrowed.contents.is_none());
        assert_eq!(borrowed.name, PLAYLIST_NAME, "metadata is still there");
    }

    #[test]
    fn playlist_items_are_paged() {
        let mock = Mock::new();
        let second = format!("{}/playlists/{PLAYLIST_ID}/items?offset=100", mock.base);
        mock.add(
            Route::new("GET", &format!("/playlists/{PLAYLIST_ID}/items"))
                .reply(200, &playlist_items_body(Some(&second))),
        );
        mock.add(
            Route::new("GET", &format!("/playlists/{PLAYLIST_ID}/items"))
                .reply(200, &paged(&[], None)),
        );
        let client = mock.client();

        let first = client.playlist_items(PLAYLIST_ID, None).expect("items");
        assert_eq!(
            first.tracks().cloned().collect::<Vec<Track>>(),
            vec![teardrop(), angel(), episode()]
        );
        let next = first.next.expect("a next page");
        let last = client
            .playlist_items(PLAYLIST_ID, Some(&next))
            .expect("items");
        assert!(last.is_empty(), "the end of the list");
    }

    /// A row trak cannot render is kept as a row and carries no track, rather
    /// than being invented or taking the whole playlist down with it.
    #[test]
    fn a_playlist_row_with_no_track_is_kept_and_renders_nothing() {
        let mock = Mock::new();
        mock.add(
            Route::new("GET", &format!("/playlists/{PLAYLIST_ID}/items"))
                .reply(200, r#"{"items":[{"added_at":"x","track":null}]}"#),
        );
        let page = mock
            .client()
            .playlist_items(PLAYLIST_ID, None)
            .expect("items");
        assert_eq!(page.items.len(), 1, "the row is still there");
        assert_eq!(page.tracks().count(), 0, "and it shows nothing");
    }

    /// A podcast episode is shaped enough to read as a track, which is why the
    /// row is not dropped: it has a title and a length.
    #[test]
    fn an_episode_reads_as_a_track_with_no_artists() {
        let parsed: TrackItem = serde_json::from_str(&format!(
            r#"{{"added_at":"x","track":{episode}}}"#,
            episode = episode_json()
        ))
        .expect("an episode is a row trak can hold");
        let track = parsed.track.expect("a track");
        assert_eq!(track, episode());
        assert_eq!(track.to_string(), "An episode");
    }

    #[test]
    fn create_playlist_posts_the_name_and_returns_the_playlist() {
        let mock = Mock::new();
        mock.add(Route::new("POST", "/me/playlists").reply(201, &playlist_body_created()));
        let client = mock.client();

        let made = client
            .create_playlist(PLAYLIST_NAME, false)
            .expect("created");

        assert_eq!(
            made,
            Playlist {
                contents: Some(PlaylistContents {
                    total: Some(0),
                    items: Vec::new()
                }),
                ..playlist_owned()
            },
            "a new playlist is empty, and says so with a count rather than a gap"
        );
        let request = &mock.requests()[0];
        assert_eq!(request.method, "POST");
        assert!(
            request
                .body
                .contains(r#""name":"Massive Attack on Repeat""#),
            "{}",
            request.body
        );
        assert!(
            request.body.contains(r#""public":false"#),
            "{}",
            request.body
        );
    }

    /// A name with a quote in it must not be able to produce a broken body: the
    /// playlist name is JSON, so it is escaped rather than interpolated.
    #[test]
    fn a_quote_in_a_playlist_name_cannot_break_the_body() {
        let mock = Mock::new();
        mock.add(Route::new("POST", "/me/playlists").reply(201, &playlist_body_created()));
        mock.client()
            .create_playlist(r#"he said "hi" & left"#, true)
            .expect("created");
        let body = &mock.requests()[0].body;
        assert!(body.contains(r#"\"hi\""#), "{body}");
        assert!(
            body.contains("&"),
            "and the ampersand stayed inside the string"
        );
    }

    #[test]
    fn add_to_playlist_sends_exactly_one_uri() {
        let mock = Mock::new();
        mock.add(Route::new("POST", &format!("/playlists/{PLAYLIST_ID}/items")).reply(200, ""));
        mock.client()
            .add_to_playlist(PLAYLIST_ID, TRACK_URI)
            .expect("added");

        let request = &mock.requests()[0];
        assert_eq!(request.method, "POST");
        assert_eq!(request.param("uris"), Some(TRACK_URI));
        assert_eq!(request.query.len(), 1, "one uri, never a list");
        assert!(request.body.is_empty(), "and no body: {:?}", request.body);
    }

    #[test]
    fn remove_from_playlist_puts_the_track_in_the_body() {
        let mock = Mock::new();
        mock.add(Route::new("DELETE", &format!("/playlists/{PLAYLIST_ID}/items")).reply(200, ""));
        mock.client()
            .remove_from_playlist(PLAYLIST_ID, TRACK_URI)
            .expect("removed");

        let request = &mock.requests()[0];
        assert_eq!(request.method, "DELETE");
        assert!(request.query.is_empty(), "{:?}", request.query);
        assert!(
            request.body.contains(&format!(r#""uri":"{TRACK_URI}""#)),
            "{}",
            request.body
        );
    }

    #[test]
    fn liked_tracks_are_the_consolidated_items_shape() {
        let mock = Mock::new();
        mock.add(Route::new("GET", "/me/tracks").reply(200, &liked_body(None)));
        let page = mock.client().liked_tracks(None).expect("liked");
        assert_eq!(
            page.tracks().cloned().collect::<Vec<Track>>(),
            vec![teardrop(), angel()]
        );
        assert!(page.next.is_none());
    }

    #[test]
    fn is_liked_reads_the_consolidated_contains_endpoint() {
        let mock = Mock::new();
        mock.add(Route::new("GET", "/me/library/contains").reply(200, "[true]"));
        mock.add(Route::new("GET", "/me/library/contains").reply(200, "[false]"));
        let client = mock.client();

        assert_eq!(client.is_liked(TRACK_URI), Ok(true));
        assert_eq!(client.is_liked(SECOND_TRACK_URI), Ok(false));

        for request in mock.requests() {
            assert_eq!(request.endpoint(), ("GET", "/me/library/contains"));
            assert_eq!(request.param("types"), Some("track"));
            let ids = request.param("ids").expect("ids");
            assert!(!ids.contains(','), "one id per request: {ids}");
            assert!(ids.starts_with("spotify:track:"), "{ids}");
        }
    }

    #[test]
    fn set_liked_writes_the_consolidated_library_endpoint() {
        let mock = Mock::new();
        mock.add(Route::new("PUT", "/me/library").reply(200, ""));
        mock.add(Route::new("DELETE", "/me/library").reply(200, ""));
        let client = mock.client();

        client.set_liked(TRACK_URI, true).expect("liked");
        client.set_liked(TRACK_URI, false).expect("unliked");

        let requests = mock.requests();
        assert_eq!(requests[0].method, "PUT");
        assert_eq!(requests[1].method, "DELETE");
        assert_eq!(requests[0].path, "/me/library");
        assert!(requests[0].body.contains(TRACK_URI), "{}", requests[0].body);
        assert!(
            requests[0].body.contains(r#""types":["track"]"#),
            "{}",
            requests[0].body
        );
        // The removed endpoints are spelled nowhere in either body.
        let bodies = format!("{}{}", requests[0].body, requests[1].body);
        assert!(!bodies.contains("/me/tracks"), "{bodies}");
    }

    #[test]
    fn the_queue_splits_now_playing_from_up_next() {
        let mock = Mock::new();
        mock.add(Route::new("GET", "/me/player/queue").reply(
            200,
            &format!(
                r#"{{"currently_playing":{teardrop},"queue":[{angel},{episode}]}}"#,
                teardrop = teardrop_json(),
                angel = angel_json(),
                episode = episode_json(),
            ),
        ));
        let queue = mock.client().queue().expect("queue");
        assert_eq!(queue.now_playing, Some(teardrop()));
        // An episode in the queue has no artists, so it reads as a track with
        // none rather than taking the whole queue down.
        assert_eq!(queue.upcoming, vec![angel(), episode()]);
    }

    #[test]
    fn saved_albums_and_followed_artists_come_back_with_their_images() {
        let mock = Mock::new();
        mock.add(Route::new("GET", "/me/albums").reply(200, &paged(&[album_json()], None)));
        mock.add(Route::new("GET", "/me/following").reply(200, &paged(&[artist_json()], None)));
        let client = mock.client();

        let albums = client.saved_albums(None).expect("saved");
        assert_eq!(albums.items, vec![album()]);
        assert_eq!(albums.items[0].images.len(), 1);

        let artists = client.followed_artists(None).expect("followed");
        assert_eq!(artists.items, vec![artist()]);
    }

    #[test]
    fn artist_albums_are_the_only_thing_an_artist_page_can_ask_for() {
        let mock = Mock::new();
        mock.add(
            Route::new("GET", &format!("/artists/{ARTIST_ID}/albums"))
                .reply(200, &paged(&[album_json()], None)),
        );
        let page = mock
            .client()
            .artist_albums(ARTIST_ID, None)
            .expect("albums");
        assert_eq!(page.items, vec![album()]);
        // The endpoint that was removed, and the reason the artist page is
        // albums-only, is not among the paths that were used.
        assert!(
            !mock
                .paths()
                .iter()
                .any(|(_, path)| path.contains("top-tracks")),
            "{:?}",
            mock.paths()
        );
    }

    #[test]
    fn an_album_and_its_tracklist_are_two_reads() {
        let mock = Mock::new();
        mock.add(Route::new("GET", &format!("/albums/{ALBUM_ID}")).reply(200, &album_json()));
        mock.add(
            Route::new("GET", &format!("/albums/{ALBUM_ID}/tracks"))
                .reply(200, &album_tracks_body(None)),
        );
        let client = mock.client();

        let read = client.album(ALBUM_ID).expect("album");
        assert_eq!(read, album());
        assert_eq!(read.total_tracks, Some(11));
        assert_eq!(read.release_date.as_deref(), Some("1998-04-20"));
        assert_eq!(
            client.album_tracks(ALBUM_ID, None).expect("tracks").items,
            vec![teardrop(), angel()]
        );

        assert_eq!(
            mock.paths(),
            vec![
                ("GET".to_string(), format!("/albums/{ALBUM_ID}")),
                ("GET".to_string(), format!("/albums/{ALBUM_ID}/tracks")),
            ]
        );
    }

    #[test]
    fn recently_played_unwraps_the_track() {
        let mock = Mock::new();
        mock.add(Route::new("GET", "/me/player/recently-played").reply(200, &recently_body(None)));
        assert_eq!(
            mock.client().recently_played(None).expect("recent").items,
            vec![teardrop(), angel()]
        );
    }

    /// A body that still carries a field dev mode removed must read exactly like
    /// one without it. This is the test that keeps a `popularity` from coming
    /// back as a dash in a row trak has no number for.
    #[test]
    fn a_removed_field_in_the_body_is_ignored_rather_than_modelled() {
        let clean_body = format!(
            r#"{{"tracks":{{"items":[{teardrop}]}},"albums":{{"items":[]}},"artists":{{"items":[]}},"playlists":{{"items":[]}}}}"#,
            teardrop = teardrop_json()
        );
        let clean = Mock::new();
        clean.add(Route::new("GET", "/search").reply(200, &clean_body));
        let expected = clean.client().search("mass").expect("search");

        // The same body with `popularity` back on the track and `followers` back
        // on the artist -- all of which lost it in dev mode.
        let noisy_body = clean_body.replace(
            r#"{"album":"#,
            r#"{"popularity":87,"label":"Melankolic","album":"#,
        );
        let noisy = Mock::new();
        noisy.add(Route::new("GET", "/search").reply(200, &noisy_body));
        let same = noisy.client().search("mass").expect("search");

        assert_eq!(expected, same, "a removed field changed nothing");
        assert!(
            !same.tracks[0].to_string().contains("87"),
            "and none of it reaches a row: {}",
            same.tracks[0]
        );
    }

    // =======================================================================
    // Every endpoint, and nothing else
    // =======================================================================

    /// The test the dev-mode removals exist for: walk every method on the trait,
    /// then read every request that produced and assert the whole set is exactly
    /// the documented one -- no batch endpoint, no removed endpoint, no
    /// top-tracks, no `popularity` in any body.
    #[test]
    fn every_request_this_build_makes_is_inside_the_documented_set() {
        let mock = Mock::new();
        for (method, path, body) in [
            ("GET", "/search", search_body()),
            ("GET", "/me/playlists", paged(&[playlist_stub_json()], None)),
            (
                "GET",
                &format!("/playlists/{PLAYLIST_ID}"),
                playlist_body_owned(None),
            ),
            (
                "GET",
                &format!("/playlists/{PLAYLIST_ID}/items"),
                playlist_items_body(None),
            ),
            ("POST", "/me/playlists", playlist_body_created()),
            (
                "POST",
                &format!("/playlists/{PLAYLIST_ID}/items"),
                String::new(),
            ),
            (
                "DELETE",
                &format!("/playlists/{PLAYLIST_ID}/items"),
                String::new(),
            ),
            ("GET", "/me/tracks", liked_body(None)),
            ("GET", "/me/library/contains", "[true]".to_string()),
            ("PUT", "/me/library", String::new()),
            ("DELETE", "/me/library", String::new()),
            (
                "GET",
                "/me/player/queue",
                format!(
                    r#"{{"currently_playing":{track},"queue":[]}}"#,
                    track = teardrop_json()
                ),
            ),
            ("POST", "/me/player/queue", String::new()),
            ("GET", "/me/albums", paged(&[album_json()], None)),
            ("GET", "/me/following", paged(&[artist_json()], None)),
            ("GET", "/me/player/recently-played", recently_body(None)),
            (
                "GET",
                &format!("/artists/{ARTIST_ID}/albums"),
                paged(&[album_json()], None),
            ),
            ("GET", &format!("/albums/{ALBUM_ID}"), album_json()),
            (
                "GET",
                &format!("/albums/{ALBUM_ID}/tracks"),
                album_tracks_body(None),
            ),
        ] {
            mock.add(Route::new(method, path).reply(200, &body));
        }
        let client = mock.client();

        for (what, result) in [
            ("search", client.search("mass").map(|_| ())),
            ("playlists", client.playlists(None).map(|_| ())),
            ("playlist", client.playlist(PLAYLIST_ID).map(|_| ())),
            (
                "playlist items",
                client.playlist_items(PLAYLIST_ID, None).map(|_| ()),
            ),
            (
                "create",
                client.create_playlist(PLAYLIST_NAME, false).map(|_| ()),
            ),
            ("add", client.add_to_playlist(PLAYLIST_ID, TRACK_URI)),
            (
                "remove",
                client.remove_from_playlist(PLAYLIST_ID, TRACK_URI),
            ),
            ("liked", client.liked_tracks(None).map(|_| ())),
            ("is liked", client.is_liked(TRACK_URI).map(|_| ())),
            ("like", client.set_liked(TRACK_URI, true)),
            ("unlike", client.set_liked(TRACK_URI, false)),
            ("enqueue", client.enqueue(TRACK_URI)),
            ("saved albums", client.saved_albums(None).map(|_| ())),
            ("followed", client.followed_artists(None).map(|_| ())),
            ("recent", client.recently_played(None).map(|_| ())),
            (
                "artist albums",
                client.artist_albums(ARTIST_ID, None).map(|_| ()),
            ),
            ("album", client.album(ALBUM_ID).map(|_| ())),
            (
                "album tracks",
                client.album_tracks(ALBUM_ID, None).map(|_| ()),
            ),
        ] {
            assert!(
                result.is_ok(),
                "{what} failed: {:?}; the request is what this test is about",
                result.err()
            );
        }
        // The one method the table above cannot hold, because its value is not a
        // page and not a bare write.
        client.queue().expect("queue");

        let mut used: Vec<(String, String)> = mock.paths();
        used.sort();
        used.dedup();
        // Both sides are sorted. The point of this test is that no endpoint
        // outside the documented set appears, and an unsorted comparison would
        // fail on ordering alone -- which is a test that punishes a reformat and
        // teaches nothing about the thing it is guarding.
        let mut expected: Vec<(String, String)> = vec![
            (
                "DELETE".to_string(),
                format!("/playlists/{PLAYLIST_ID}/items"),
            ),
            ("DELETE".to_string(), "/me/library".to_string()),
            ("GET".to_string(), format!("/albums/{ALBUM_ID}")),
            ("GET".to_string(), format!("/albums/{ALBUM_ID}/tracks")),
            ("GET".to_string(), format!("/artists/{ARTIST_ID}/albums")),
            ("GET".to_string(), "/me/albums".to_string()),
            ("GET".to_string(), "/me/following".to_string()),
            ("GET".to_string(), "/me/library/contains".to_string()),
            ("GET".to_string(), "/me/player/queue".to_string()),
            ("GET".to_string(), "/me/player/recently-played".to_string()),
            ("GET".to_string(), "/me/playlists".to_string()),
            ("GET".to_string(), "/me/tracks".to_string()),
            ("GET".to_string(), "/search".to_string()),
            ("GET".to_string(), format!("/playlists/{PLAYLIST_ID}")),
            ("GET".to_string(), format!("/playlists/{PLAYLIST_ID}/items")),
            ("POST".to_string(), "/me/playlists".to_string()),
            ("POST".to_string(), "/me/player/queue".to_string()),
            (
                "POST".to_string(),
                format!("/playlists/{PLAYLIST_ID}/items"),
            ),
            ("PUT".to_string(), "/me/library".to_string()),
        ];
        expected.sort();
        assert_eq!(
            used, expected,
            "the set of endpoints trak touches, with nothing extra"
        );

        for request in mock.requests() {
            assert!(
                allowed(request.method.as_str(), request.path.as_str()).is_some(),
                "{} {} is not on the list",
                request.method,
                request.path
            );
            assert_eq!(
                request.authorization.as_deref(),
                Some("Bearer access-abc"),
                "every request carries the bearer token"
            );
        }
        let bodies = mock
            .requests()
            .iter()
            .map(|request| request.body.clone())
            .collect::<Vec<String>>()
            .join("");
        assert!(!bodies.contains("popularity"), "{bodies}");
    }

    // =======================================================================
    // Errors
    // =======================================================================

    fn failing(method: &str, status: u16, body: &str) -> Mock {
        let mock = Mock::new();
        mock.add(Route::anywhere(method).reply(status, body));
        mock
    }

    #[test]
    fn a_429_with_retry_after_holds_the_next_request_off() {
        let mock = Mock::new();
        mock.add(
            Route::new("GET", "/me/player/queue")
                .header("Retry-After", "45")
                .reply(
                    429,
                    r#"{"error":{"status":429,"message":"Too many requests"}}"#,
                ),
        );
        // A route behind it, so a request that slipped past the gate would
        // succeed and this test would notice.
        mock.add(
            Route::new("GET", "/me/player/queue")
                .reply(200, r#"{"currently_playing":null,"queue":[]}"#),
        );
        let client = mock.client();

        let error = client.queue().expect_err("rate limited");
        assert_eq!(
            error,
            ApiError::RateLimited {
                retry_after: Duration::from_secs(45)
            }
        );
        assert_eq!(error.notice(), "spotify: rate limited, retrying in 45s");

        // Everything inside the window is refused without a request.
        assert!(matches!(
            client.album(ALBUM_ID),
            Err(ApiError::RateLimited { .. })
        ));
        assert!(client.search("mass").is_err());
        assert_eq!(mock.requests().len(), 1, "only the first request went out");
        assert!(error.is_transient());
    }

    /// "The header *will normally* include a `Retry-After`" -- so a 429 without
    /// one has to mean something, and it must not mean "retry now".
    #[test]
    fn a_429_without_retry_after_falls_back_to_the_cap() {
        let mock = failing(
            "GET",
            429,
            r#"{"error":{"status":429,"message":"Too many requests"}}"#,
        );
        assert_eq!(
            mock.client().queue(),
            Err(ApiError::RateLimited {
                retry_after: RETRY_AFTER_CAP
            })
        );
    }

    #[test]
    fn a_retry_after_beyond_the_cap_is_clipped_to_it() {
        assert_eq!(RETRY_AFTER_CAP, Duration::from_secs(60));
        let mock = Mock::new();
        mock.add(
            Route::new("GET", "/me/player/queue")
                .header("Retry-After", "86400")
                .reply(429, "{}"),
        );
        assert_eq!(
            mock.client().queue(),
            Err(ApiError::RateLimited {
                retry_after: RETRY_AFTER_CAP
            }),
            "a day is not a wait a worker thread can be asked to hold"
        );
    }

    /// And the window really does close, which is the whole of the back-off.
    #[test]
    fn the_rate_limit_window_closes_and_requests_work_again() {
        let mock = Mock::new();
        mock.add(
            Route::new("GET", "/me/player/queue")
                .header("Retry-After", "1")
                .reply(429, "{}"),
        );
        mock.add(
            Route::new("GET", "/me/player/queue")
                .reply(200, r#"{"currently_playing":null,"queue":[]}"#),
        );
        let client = mock.client().with_retry_cap(Duration::from_millis(80));

        assert!(client.queue().is_err());
        assert!(client.queue().is_err(), "still inside the window");
        assert_eq!(mock.requests().len(), 1);
        thread::sleep(Duration::from_millis(120));
        assert!(
            client.queue().is_ok(),
            "and after it, the API is asked again"
        );
        assert_eq!(mock.requests().len(), 2);
    }

    /// The other 429: the development-mode quota for the whole developer account
    /// is spent, which is enforced separately from a rate limit and does not
    /// clear on its own.
    #[test]
    fn a_quota_exhausted_429_is_not_a_rate_limit() {
        let mock = Mock::new();
        mock.add(
            Route::new("GET", "/me/player/queue")
                .header("Retry-After", "5")
                .reply(
                    429,
                    r#"{"error":{"status":429,"message":"Too many requests","reason":"QUOTA_EXCEEDED"}}"#,
                ),
        );
        let error = mock.client().queue().expect_err("no quota");
        assert_eq!(error, ApiError::QuotaExceeded);
        assert_eq!(
            error.notice(),
            "spotify: the developer quota is spent — try again later"
        );
        assert!(error.is_transient());
    }

    /// The same word in the prose must not be mistaken for the field.
    #[test]
    fn quota_exceeded_is_read_from_the_reason_field_only() {
        assert!(quota_exceeded(
            r#"{"error":{"status":429,"message":"Too many requests","reason":"QUOTA_EXCEEDED"}}"#
        ));
        assert!(!quota_exceeded(
            r#"{"error":{"status":429,"message":"QUOTA_EXCEEDED for this client"}}"#
        ));
        assert!(!quota_exceeded("not json at all"));
        assert!(!quota_exceeded(r#"{"error":{}}"#));
    }

    /// 403 on anything but the queue is the allowlist, and the notice has to say
    /// so: the user cannot fix a 403 by trying again.
    #[test]
    fn a_403_is_the_allowlist() {
        let mock = failing(
            "GET",
            403,
            r#"{"error":{"status":403,"message":"Forbidden"}}"#,
        );
        let error = mock.client().search("mass").expect_err("forbidden");
        assert_eq!(error, ApiError::NotAllowlisted);
        assert!(
            error.notice().contains("allowlist"),
            "the notice must name the cause: {}",
            error.notice()
        );
        assert!(error.needs_relogin());
    }

    /// And the same status on the one endpoint Spotify documents as Premium-only.
    #[test]
    fn a_403_on_the_queue_is_the_premium_limit() {
        let mock = failing(
            "POST",
            403,
            r#"{"error":{"status":403,"message":"Forbidden"}}"#,
        );
        let error = mock.client().enqueue(TRACK_URI).expect_err("forbidden");
        assert_eq!(error, ApiError::PremiumOnly);
        assert!(error.notice().contains("Premium"), "{}", error.notice());
        assert!(
            !error.needs_relogin(),
            "a Premium limit is not something a new login fixes"
        );
    }

    #[test]
    fn a_401_asks_for_a_new_login() {
        let mock = failing(
            "GET",
            401,
            r#"{"error":{"status":401,"message":"No token provided"}}"#,
        );
        let error = mock.client().queue().expect_err("unauthorised");
        assert_eq!(error, ApiError::Unauthorized);
        assert!(error.needs_relogin());
    }

    #[test]
    fn a_404_is_not_found() {
        let mock = failing(
            "GET",
            404,
            r#"{"error":{"status":404,"message":"Not found"}}"#,
        );
        assert_eq!(
            mock.client().album("no-such-album"),
            Err(ApiError::NotFound)
        );
    }

    #[test]
    fn an_unexpected_status_is_kept_as_a_status() {
        let mock = failing(
            "GET",
            503,
            r#"{"error":{"status":503,"message":"Service unavailable"}}"#,
        );
        let error = mock.client().queue().expect_err("unavailable");
        assert_eq!(error, ApiError::Status(503));
        assert_eq!(error.notice(), "spotify: answered HTTP 503");
        assert!(error.is_transient());
    }

    #[test]
    fn a_body_that_is_not_json_is_malformed() {
        let mock = failing("GET", 200, "<html>maintenance</html>");
        assert_eq!(mock.client().queue(), Err(ApiError::Malformed));
    }

    /// A body cut in half: the length is right, so the read succeeds and the
    /// parse is what fails.
    #[test]
    fn a_body_cut_in_half_is_malformed() {
        let full = search_body();
        let cut = &full[..full.len() / 2];
        let mock = Mock::new();
        mock.add(Route::new("GET", "/search").reply(200, cut));
        assert_eq!(mock.client().search("mass"), Err(ApiError::Malformed));
    }

    /// A 200 whose body is missing something trak needs. Every model has at least
    /// one required field, which is what makes this possible.
    #[test]
    fn a_response_missing_a_field_trak_needs_is_malformed() {
        let no_name =
            r#"{"id":"5nMdc39z78kifAc5WXv9Yj","uri":"spotify:album:5nMdc39z78kifAc5WXv9Yj"}"#;
        let mock = Mock::new();
        mock.add(Route::new("GET", "/me/albums").reply(200, &paged(&[no_name.to_string()], None)));
        assert_eq!(
            mock.client().saved_albums(None),
            Err(ApiError::Malformed),
            "an album with no name is not an album trak can show"
        );
    }

    /// An empty `contains` array is not an answer of "no": it would make `f`
    /// save a track that is already saved.
    #[test]
    fn an_empty_contains_array_is_malformed_rather_than_false() {
        let mock = failing("GET", 200, "[]");
        assert_eq!(mock.client().is_liked(TRACK_URI), Err(ApiError::Malformed));
    }

    #[test]
    fn a_body_too_large_to_be_trak_s_answer_is_malformed() {
        let filler = "x".repeat(MAX_BYTES as usize + 1);
        let body = format!(r#"{{"tracks":{{"items":[{filler}]}}}}"#);
        let mock = failing("GET", 200, &body);
        assert_eq!(mock.client().search("mass"), Err(ApiError::Malformed));
    }

    #[test]
    fn nothing_listening_is_unreachable() {
        // Port 1 on loopback: nothing is bound there, so this is a refused
        // connection rather than a guess about a host that might answer.
        let client = SpotifyLibrary::at("http://127.0.0.1:1/v1", &token());
        assert_eq!(client.queue(), Err(ApiError::Unreachable));
    }

    #[test]
    fn a_connection_closed_before_the_response_is_unreachable() {
        let mock = Mock::new();
        mock.add(Route::cutting());
        assert_eq!(mock.client().queue(), Err(ApiError::Unreachable));
    }

    #[test]
    fn a_server_that_says_nothing_is_a_timeout() {
        let mock = Mock::new();
        mock.add(Route::stalling());
        let client = mock.client().with_timeout(Duration::from_millis(100));
        let error = client.queue().expect_err("no answer");
        assert_eq!(error, ApiError::Timeout);
        assert!(error.is_transient());
        assert!(!error.needs_relogin());
    }

    /// Every variant reaches a toast, so every one of them has to be a line that
    /// says something a person can act on.
    #[test]
    fn every_error_has_a_one_line_notice_that_says_something() {
        for error in [
            ApiError::NotConnected,
            ApiError::Unauthorized,
            ApiError::NotAllowlisted,
            ApiError::PremiumOnly,
            ApiError::RateLimited {
                retry_after: Duration::from_secs(3),
            },
            ApiError::QuotaExceeded,
            ApiError::NotFound,
            ApiError::Malformed,
            ApiError::Unreachable,
            ApiError::Timeout,
            ApiError::Status(500),
        ] {
            let notice = error.notice();
            assert!(!notice.contains('\n'), "{notice:?} wraps");
            assert!(notice.starts_with("spotify: "), "{notice:?}");
            assert!(
                notice.len() > "spotify: ".len() + 4,
                "{notice:?} says nothing"
            );
            assert!(!notice.contains("SpotifyError"), "{notice:?}");
        }
        // The two 403s mean different things to a person and must not read alike.
        assert_ne!(
            ApiError::NotAllowlisted.notice(),
            ApiError::PremiumOnly.notice()
        );
        assert_ne!(
            ApiError::RateLimited {
                retry_after: Duration::from_secs(1)
            }
            .notice(),
            ApiError::RateLimited {
                retry_after: Duration::from_secs(9)
            }
            .notice(),
        );
    }

    // =======================================================================
    // The cache
    // =======================================================================

    #[test]
    fn a_second_read_of_the_same_album_makes_no_request() {
        let mock = Mock::new();
        mock.add(Route::new("GET", &format!("/albums/{ALBUM_ID}")).reply(200, &album_json()));
        let client = mock.client();

        assert_eq!(client.album(ALBUM_ID).expect("first"), album());
        assert_eq!(client.album(ALBUM_ID).expect("cached"), album());
        assert_eq!(client.album(ALBUM_ID).expect("cached"), album());
        assert_eq!(mock.requests().len(), 1, "one request for three reads");
        assert_eq!(client.cached_len(), 1);
    }

    #[test]
    fn a_search_is_cached_by_its_exact_query() {
        let mock = Mock::new();
        mock.add(
            Route::new("GET", "/search")
                .times(2)
                .reply(200, &search_body()),
        );
        let client = mock.client();

        client.search("mass").expect("first");
        client.search("mass").expect("cached");
        client.search("teardrop").expect("a different query");
        client.search("teardrop").expect("cached");

        assert_eq!(
            mock.requests()
                .iter()
                .map(|request| request.param("q").map(str::to_string))
                .collect::<Vec<Option<String>>>(),
            vec![Some("mass".to_string()), Some("teardrop".to_string())],
        );
    }

    /// The bound is enforced, and the victim is the least recently *used* entry
    /// rather than the oldest: the album the user is looking at, and then backed
    /// out of, is not what a browsing session should lose.
    #[test]
    fn the_cache_never_grows_past_its_bound_and_evicts_least_recently_used() {
        let mock = Mock::new();
        for n in 0..CACHE_CAPACITY + 2 {
            let id = format!("id{n}");
            // Twice, because the test reads two of them a second time and a
            // cache miss has to be able to reach the network.
            mock.add(
                Route::new("GET", &format!("/albums/{id}"))
                    .times(2)
                    .reply(200, &album_body(&id)),
            );
        }
        let client = mock.client();
        for n in 0..=CACHE_CAPACITY {
            client.album(&format!("id{n}")).expect("album");
        }
        assert_eq!(client.cached_len(), CACHE_CAPACITY, "it is exactly full");

        // `id0` was read first; reading it again makes it the newest.
        client.album("id0").expect("album");
        // One more pushes the least recently used -- `id1`, read second and not
        // since -- out of the table.
        let newest = format!("id{}", CACHE_CAPACITY + 1);
        client.album(&newest).expect("album");
        assert_eq!(
            client.cached_len(),
            CACHE_CAPACITY,
            "and it is still bounded"
        );

        let before = mock.requests().len();
        client.album("id0").expect("still cached");
        assert_eq!(
            mock.requests().len(),
            before,
            "the entry that was touched survived"
        );
        client.album("id1").expect("a miss");
        assert_eq!(
            mock.requests().len(),
            before + 1,
            "and the least recently used one did not"
        );
    }

    /// The other half of the cache rule: anything the user owns, edits or is
    /// playing is asked for again every time. A cached playlist would make TODO
    /// 7.11's "remove from a playlist" look like it did nothing.
    #[test]
    fn live_data_is_never_cached() {
        let mock = Mock::new();
        mock.add(
            Route::new("GET", "/me/player/queue")
                .times(2)
                .reply(200, r#"{"currently_playing":null,"queue":[]}"#),
        );
        mock.add(
            Route::new("GET", &format!("/playlists/{PLAYLIST_ID}/items"))
                .times(2)
                .reply(200, &playlist_items_body(None)),
        );
        mock.add(
            Route::new("GET", &format!("/playlists/{PLAYLIST_ID}"))
                .times(2)
                .reply(200, &playlist_body_owned(None)),
        );
        mock.add(
            Route::new("GET", "/me/tracks")
                .times(2)
                .reply(200, &liked_body(None)),
        );
        mock.add(
            Route::new("GET", "/me/albums")
                .times(2)
                .reply(200, &paged(&[album_json()], None)),
        );
        mock.add(
            Route::new("GET", "/me/following")
                .times(2)
                .reply(200, &paged(&[artist_json()], None)),
        );
        mock.add(
            Route::new("GET", "/me/player/recently-played")
                .times(2)
                .reply(200, &recently_body(None)),
        );
        let client = mock.client();

        for _ in 0..2 {
            client.queue().expect("queue");
            client.playlist_items(PLAYLIST_ID, None).expect("items");
            client.playlist(PLAYLIST_ID).expect("playlist");
            client.liked_tracks(None).expect("liked");
            client.saved_albums(None).expect("albums");
            client.followed_artists(None).expect("followed");
            client.recently_played(None).expect("recent");
        }

        assert_eq!(
            mock.requests().len(),
            14,
            "every one of those is asked for twice: {:?}",
            mock.paths()
        );
        assert_eq!(client.cached_len(), 0);
    }

    /// A renewed access token does not invalidate the catalogue: the key is the
    /// request, not the session.
    #[test]
    fn a_renewed_token_does_not_invalidate_the_cache_but_does_change_the_wire() {
        let mock = Mock::new();
        mock.add(Route::new("GET", &format!("/albums/{ALBUM_ID}")).reply(200, &album_json()));
        mock.add(
            Route::new("GET", "/me/player/queue")
                .reply(200, r#"{"currently_playing":null,"queue":[]}"#),
        );
        let client = mock.client();
        client.album(ALBUM_ID).expect("album");

        client.set_token(&token_with("access-second"));
        client.album(ALBUM_ID).expect("album");
        assert_eq!(mock.requests().len(), 1, "the cache survives the renewal");

        client.queue().expect("queue");
        assert_eq!(
            mock.requests()[1].authorization.as_deref(),
            Some("Bearer access-second"),
            "and the new token is the one that goes out"
        );
    }

    // =======================================================================
    // The fake
    // =======================================================================

    /// The comparison the whole module exists to enable: the same input through
    /// both implementations gives the same thing, field for field and character
    /// for character.
    #[test]
    fn the_fake_and_the_real_client_agree_on_a_search_result() {
        let mock = Mock::new();
        mock.add(Route::new("GET", "/search").reply(200, &search_body()));
        let real = mock.client().search("mass").expect("real search");

        let fake = FakeLibrary::with_catalogue(
            vec![teardrop(), angel()],
            vec![album()],
            vec![artist()],
            vec![playlist_stub()],
        )
        .search("mass")
        .expect("fake search");

        assert_eq!(real, fake, "the two disagree about a search result");
        // And the text a row would be drawn with is identical, not merely equal
        // field by field.
        assert_eq!(
            real.tracks
                .iter()
                .map(Track::to_string)
                .collect::<Vec<String>>(),
            fake.tracks
                .iter()
                .map(Track::to_string)
                .collect::<Vec<String>>(),
        );
        assert_eq!(real.tracks[0].to_string(), "Massive Attack — Teardrop");
    }

    /// The same, across every tab. Each method is compared against a value
    /// written out by hand, so the fake cannot pass by agreeing with a bug in the
    /// parser.
    #[test]
    fn the_fake_and_the_real_client_agree_across_every_tab() {
        let mock = Mock::new();
        mock.add(Route::new("GET", "/search").reply(200, &search_body()));
        mock.add(
            Route::new("GET", "/me/playlists").reply(200, &paged(&[playlist_stub_json()], None)),
        );
        mock.add(
            Route::new("GET", &format!("/playlists/{PLAYLIST_ID}"))
                .reply(200, &playlist_body_owned(None)),
        );
        mock.add(
            Route::new("GET", &format!("/playlists/{PLAYLIST_ID}/items"))
                .reply(200, &playlist_items_body(None)),
        );
        mock.add(Route::new("POST", "/me/playlists").reply(201, &playlist_body_created()));
        mock.add(Route::new("POST", &format!("/playlists/{PLAYLIST_ID}/items")).reply(200, ""));
        mock.add(Route::new("GET", "/me/tracks").reply(200, &liked_body(None)));
        mock.add(Route::new("GET", "/me/library/contains").reply(200, "[true]"));
        mock.add(Route::new("PUT", "/me/library").reply(200, ""));
        mock.add(Route::new("GET", "/me/player/queue").reply(
            200,
            &format!(
                r#"{{"currently_playing":{teardrop},"queue":[{angel}]}}"#,
                teardrop = teardrop_json(),
                angel = angel_json()
            ),
        ));
        mock.add(Route::new("POST", "/me/player/queue").reply(200, ""));
        mock.add(Route::new("GET", "/me/albums").reply(200, &paged(&[album_json()], None)));
        mock.add(Route::new("GET", "/me/following").reply(200, &paged(&[artist_json()], None)));
        mock.add(Route::new("GET", "/me/player/recently-played").reply(200, &recently_body(None)));
        mock.add(
            Route::new("GET", &format!("/artists/{ARTIST_ID}/albums"))
                .reply(200, &paged(&[album_json()], None)),
        );
        mock.add(Route::new("GET", &format!("/albums/{ALBUM_ID}")).reply(200, &album_json()));
        mock.add(
            Route::new("GET", &format!("/albums/{ALBUM_ID}/tracks"))
                .reply(200, &album_tracks_body(None)),
        );

        let real = mock.client();
        let fake = FakeLibrary::with_catalogue(
            vec![teardrop(), angel()],
            vec![album()],
            vec![artist()],
            vec![playlist_stub()],
        )
        .with_contents(PLAYLIST_ID, vec![teardrop(), angel(), episode()])
        .with_library(
            vec![teardrop(), angel()],
            vec![teardrop(), angel()],
            vec![album()],
            vec![artist()],
            vec![teardrop(), angel()],
        );
        fake.seed_liked(&[TRACK_URI, SECOND_TRACK_URI]);

        /// One comparison, so every method in the sweep is checked the same way
        /// and a failure names the tab it was about.
        macro_rules! same {
            ($what:expr, $real:expr, $fake:expr $(,)?) => {
                assert_eq!(
                    $real, $fake,
                    "the fake and the real client disagree about {}",
                    $what
                )
            };
        }

        same!(
            "playlists",
            real.playlists(None).expect("real").items,
            fake.playlists(None).expect("fake").items,
        );
        same!(
            "playlist detail",
            real.playlist(PLAYLIST_ID).expect("real"),
            fake.playlist(PLAYLIST_ID).expect("fake"),
        );
        same!(
            "playlist items",
            real.playlist_items(PLAYLIST_ID, None)
                .expect("real")
                .tracks()
                .cloned()
                .collect::<Vec<Track>>(),
            fake.playlist_items(PLAYLIST_ID, None)
                .expect("fake")
                .tracks()
                .cloned()
                .collect::<Vec<Track>>(),
        );
        same!(
            "liked tracks",
            real.liked_tracks(None)
                .expect("real")
                .tracks()
                .cloned()
                .collect::<Vec<Track>>(),
            fake.liked_tracks(None)
                .expect("fake")
                .tracks()
                .cloned()
                .collect::<Vec<Track>>(),
        );
        same!(
            "is liked",
            real.is_liked(TRACK_URI),
            fake.is_liked(TRACK_URI),
        );
        same!(
            "queue",
            real.queue().expect("real"),
            fake.queue().expect("fake")
        );
        same!(
            "saved albums",
            real.saved_albums(None).expect("real").items,
            fake.saved_albums(None).expect("fake").items,
        );
        same!(
            "followed artists",
            real.followed_artists(None).expect("real").items,
            fake.followed_artists(None).expect("fake").items,
        );
        same!(
            "recently played",
            real.recently_played(None).expect("real").items,
            fake.recently_played(None).expect("fake").items,
        );
        same!(
            "artist albums",
            real.artist_albums(ARTIST_ID, None).expect("real").items,
            fake.artist_albums(ARTIST_ID, None).expect("fake").items,
        );
        same!(
            "album",
            real.album(ALBUM_ID).expect("real"),
            fake.album(ALBUM_ID).expect("fake"),
        );
        same!(
            "album tracks",
            real.album_tracks(ALBUM_ID, None).expect("real").items,
            fake.album_tracks(ALBUM_ID, None).expect("fake").items,
        );

        // And a created playlist is the same shape from both, minus the id, which
        // is the server's to choose.
        let made_real = real.create_playlist(PLAYLIST_NAME, false).expect("real");
        let made_fake = fake.create_playlist(PLAYLIST_NAME, false).expect("fake");
        assert_eq!(made_real.name, made_fake.name);
        assert_eq!(made_real.contents, made_fake.contents);
        assert_eq!(made_real.uri, format!("spotify:playlist:{}", made_real.id));
    }

    /// Both continuations work, even though the payloads are necessarily
    /// different -- the real one is Spotify's `next` URL and the fake's an index.
    /// What a caller sees is the same: `Some`, then a second page, then the end.
    #[test]
    fn a_continuation_works_the_same_way_through_both_implementations() {
        let mock = Mock::new();
        let second = format!("{}/me/tracks?offset=100", mock.base);
        mock.add(Route::new("GET", "/me/tracks").reply(200, &liked_body(Some(&second))));
        mock.add(Route::new("GET", "/me/tracks").reply(200, &paged(&[], None)));
        let real = mock.client();

        let fake = FakeLibrary::with_catalogue(vec![teardrop(); 25], vec![], vec![], vec![])
            .with_library(vec![teardrop(); 25], vec![], vec![], vec![], vec![]);

        for library in [&real as &dyn Library, &fake as &dyn Library] {
            let first = library.liked_tracks(None).expect("page one");
            assert!(!first.items.is_empty(), "page one has rows");
            assert!(first.next.is_some(), "and there is another page");
            let second_page = library.liked_tracks(first.next.as_ref()).expect("page two");
            assert!(
                second_page.next.is_none(),
                "and the second page knows it is the last"
            );
        }
    }

    /// Reads never write, and a write is recorded exactly once, which is how TODO
    /// 7.7's `f` and 7.8's `A` are tested without a server.
    #[test]
    fn the_fake_records_writes_and_reads_are_read_only() {
        let fake = FakeLibrary::with_catalogue(
            vec![teardrop(), angel()],
            vec![album()],
            vec![artist()],
            vec![playlist_stub()],
        )
        .with_contents(PLAYLIST_ID, vec![teardrop(), angel()])
        .with_library(
            vec![teardrop()],
            vec![teardrop()],
            vec![album()],
            vec![artist()],
            vec![],
        );
        fake.seed_liked(&[TRACK_URI]);

        fake.search("mass").expect("search");
        fake.playlists(None).expect("playlists");
        fake.playlist(PLAYLIST_ID).expect("playlist");
        fake.liked_tracks(None).expect("liked");
        fake.is_liked(TRACK_URI).expect("is liked");
        fake.queue().expect("queue");
        fake.album(ALBUM_ID).expect("album");
        fake.album_tracks(ALBUM_ID, None).expect("album tracks");
        fake.artist_albums(ARTIST_ID, None).expect("artist albums");
        assert!(
            fake.writes().is_empty(),
            "a read wrote something: {:?}",
            fake.writes()
        );

        fake.set_liked(TRACK_URI, false).expect("unlike");
        fake.enqueue(SECOND_TRACK_URI).expect("enqueue");
        fake.add_to_playlist(PLAYLIST_ID, TRACK_URI).expect("add");
        fake.remove_from_playlist(PLAYLIST_ID, SECOND_TRACK_URI)
            .expect("remove");
        fake.create_playlist("New", false).expect("create");
        assert_eq!(
            fake.writes(),
            vec![
                "set_liked",
                "enqueue",
                "add_to_playlist",
                "remove_from_playlist",
                "create_playlist",
            ]
        );

        // And the writes really happened, so a tab can re-read and see it.
        assert_eq!(fake.is_liked(TRACK_URI), Ok(false));
        assert_eq!(fake.queue().expect("queue").upcoming, vec![angel()]);
        assert_eq!(
            fake.playlist_items(PLAYLIST_ID, None)
                .expect("items")
                .items
                .len(),
            2
        );
    }

    /// Setting a like that is already set, or clearing one that is not, writes
    /// once and changes nothing -- the same as the API.
    #[test]
    fn a_no_op_like_write_is_still_one_write_and_no_change() {
        let fake = FakeLibrary::with_catalogue(vec![teardrop()], vec![], vec![], vec![]);
        fake.seed_liked(&[TRACK_URI]);
        fake.set_liked(TRACK_URI, true).expect("already liked");
        assert_eq!(fake.is_liked(TRACK_URI), Ok(true));
        fake.set_liked("spotify:track:nothing", false)
            .expect("already unliked");
        fake.clear_writes();
        assert!(fake.writes().is_empty());
    }

    #[test]
    fn a_created_playlist_has_an_id_and_is_not_public_by_default() {
        let fake = FakeLibrary::new();
        let first = fake.create_playlist("One", false).expect("created");
        let second = fake.create_playlist("Two", false).expect("created");
        assert_eq!(first.id, "fake-playlist-1");
        assert_eq!(second.id, "fake-playlist-2");
        assert_eq!(first.uri, "spotify:playlist:fake-playlist-1");
        assert!(
            fake.playlists(None).expect("playlists").items.is_empty(),
            "a private playlist is not in the list"
        );
        let public = fake.create_playlist("Three", true).expect("created");
        assert_eq!(fake.playlists(None).expect("playlists").items, vec![public]);
    }

    /// The fake is how TODO 7.6–7.13 test a toast without a server.
    #[test]
    fn the_fake_can_be_told_to_fail_so_a_toast_can_be_tested() {
        let fake = FakeLibrary::failing(ApiError::PremiumOnly);
        let error = fake.enqueue(TRACK_URI).expect_err("no premium");
        assert_eq!(error.notice(), ApiError::PremiumOnly.notice());
        // Every read fails the same way, so a tab showing "no results" can be
        // told apart from one showing "not connected".
        assert_eq!(fake.search("mass"), Err(ApiError::PremiumOnly));
        assert_eq!(fake.queue(), Err(ApiError::PremiumOnly));
        assert!(
            fake.writes().is_empty(),
            "and a call that failed records nothing: {:?}",
            fake.writes()
        );

        fake.set_error(None);
        assert!(
            fake.queue().is_ok(),
            "and it recovers when the error is cleared"
        );
    }

    // =======================================================================
    // The best match (TODO 7.12)
    // =======================================================================

    /// One row with just a name, so a pick is about the name and nothing else.
    fn named(name: &str) -> Track {
        Track {
            name: name.to_string(),
            ..teardrop()
        }
    }

    /// The whole point of the tiebreak: the row *named* after the query is not
    /// the first result, and must still win.
    #[test]
    fn the_best_match_is_the_first_row_whose_name_contains_the_query() {
        let rows = [named("Teardrop"), named("Angel"), named("Mezzanine")];
        let pick = best_match(&rows, "mezzanine", |t| &t.name).expect("a pick");
        assert_eq!(pick.name, "Mezzanine");
    }

    /// Nothing on Mezzanine is named "mass" — the artist field is what matched —
    /// so the ranking Spotify already did is the answer, exactly as it was for
    /// shpotify's `limit=1` search.
    #[test]
    fn the_best_match_falls_back_to_spotifys_own_first_result() {
        let rows = [named("Teardrop"), named("Angel")];
        let pick = best_match(&rows, "mass", |t| &t.name).expect("a pick");
        assert_eq!(pick.name, "Teardrop");
    }

    #[test]
    fn the_best_match_ignores_case_and_space_and_finds_nothing_in_no_rows() {
        let rows = [named("Angel"), named("Teardrop")];
        let pick = best_match(&rows, "  TEARDROP ", |t| &t.name).expect("a pick");
        assert_eq!(pick.name, "Teardrop");
        assert!(best_match(&[], "anything", |t: &Track| &t.name).is_none());
    }

    /// The catalogue behind the binary's `--fake-library`: every group answers,
    /// and the one query that separates the tiebreak from a plain "first result"
    /// behaves there too.
    #[test]
    fn the_seeded_catalogue_answers_every_group() {
        let results = FakeLibrary::seeded().search("mass").expect("search");
        assert_eq!(results.tracks.len(), 3, "{results:?}");
        assert_eq!(results.albums.len(), 1);
        assert_eq!(results.artists.len(), 1);
        assert_eq!(results.playlists.len(), 1);

        let for_name = FakeLibrary::seeded().search("mezzanine").expect("search");
        // All three tracks match the query through their album, and the song
        // actually named Mezzanine is the last of them.
        assert_eq!(for_name.tracks.len(), 3);
        let pick = best_match(&for_name.tracks, "mezzanine", |t| &t.name).expect("pick");
        assert_eq!(pick.name, "Mezzanine");
    }

    /// The trait has to work as `dyn Library`, because that is how the TUI holds
    /// it behind the worker.
    #[test]
    fn the_trait_is_usable_as_a_trait_object_and_is_shareable() {
        fn assert_send<T: Send + Sync + 'static>() {}
        assert_send::<SpotifyLibrary>();
        assert_send::<FakeLibrary>();

        let mock = Mock::new();
        mock.add(Route::new("GET", "/me/player/queue").reply(
            200,
            &format!(
                r#"{{"currently_playing":{teardrop},"queue":[]}}"#,
                teardrop = teardrop_json()
            ),
        ));
        let libraries: Vec<Box<dyn Library>> = vec![
            Box::new(mock.client()),
            Box::new(FakeLibrary::new().with_library(
                vec![],
                vec![teardrop()],
                vec![],
                vec![],
                vec![],
            )),
        ];
        for library in libraries {
            assert_eq!(
                library.queue().expect("queue").now_playing,
                Some(teardrop())
            );
        }
    }

    #[test]
    fn a_track_row_says_its_artists_and_its_title() {
        assert_eq!(teardrop().to_string(), "Massive Attack — Teardrop");
        assert_eq!(angel().to_string(), "Massive Attack — Angel");
        // Two artists, joined, and a track with none does not print a separator
        // with nothing beside it.
        let duet = Track {
            artists: vec![
                Artist {
                    name: "A".into(),
                    ..artist_stub()
                },
                Artist {
                    name: "B".into(),
                    ..artist_stub()
                },
            ],
            ..teardrop()
        };
        assert_eq!(duet.to_string(), "A, B — Teardrop");
        assert_eq!(
            Track {
                artists: Vec::new(),
                ..teardrop()
            }
            .to_string(),
            "Teardrop"
        );
        assert_eq!(episode().to_string(), "An episode");
        assert_eq!(teardrop().duration_secs(), 301);
    }

    #[test]
    fn a_page_with_nothing_on_it_is_empty() {
        let page = Page::<Track>::empty();
        assert!(page.is_empty());
        assert_eq!(
            page,
            Page {
                items: Vec::new(),
                next: None
            }
        );
    }

    /// Nothing a body says can make trak panic. Every model is walked with the
    /// hostile shapes: wrong types, empty strings, and a `next` that is not a URL.
    #[test]
    fn no_response_shape_panics() {
        for body in [
            "",
            "null",
            "[]",
            "{}",
            r#"{"id":null}"#,
            r#"{"id":[],"name":{}}"#,
            r#"{"items":"not an array"}"#,
            r#"{"items":[{"track":7}]}"#,
            r#"{"next":7,"items":[]}"#,
            r#"{"images":[{"url":null}]}"#,
            r#"{"artists":[null]}"#,
            r#"{"duration_ms":"301000"}"#,
            r#"{"items":[{"added_at":{},"track":{"id":"a"}}]}"#,
        ] {
            for path in ["/search", "/me/tracks", "/me/player/queue", "/me/albums"] {
                let mock = Mock::new();
                mock.add(Route::new("GET", path).reply(200, body));
                let client = mock.client();
                let _ = client.search("x");
                let _ = client.liked_tracks(None);
                let _ = client.queue();
                let _ = client.saved_albums(None);
            }
        }
        // And the same through the fake's own inputs.
        let fake = FakeLibrary::new();
        assert!(fake.search("\u{1f600}").is_ok());
        assert!(fake.search("\" OR 1=1 --").is_ok());
        assert_eq!(fake.album(""), Err(ApiError::NotFound));
        assert_eq!(fake.is_liked(""), Ok(false));
    }

    /// The token helper exists so a test never needs a credential; this pins that
    /// it is a made-up one.
    #[test]
    fn the_test_token_is_a_made_up_one() {
        assert_eq!(token().access_token(), "access-abc");
        assert_eq!(token().refresh_token(), "refresh-xyz");
        assert!(
            !token().access_stale(system_now()),
            "and it is not expired, so nothing in this file needs a refresh"
        );
    }
}
