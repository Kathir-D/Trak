//! The TUI's state and its update function.
//!
//! `update` is a pure `(App, Event) -> App`. Nothing here draws, and nothing here
//! talks to Spotify: a key that needs a write becomes a `PlayerCommand`, which the
//! event loop runs on a worker and reports back as an `Event`. That split is what
//! makes the whole thing testable with no terminal and no Spotify
//! (ARCHITECTURE, "Pure state + pure render").
//!
//! Reads are the exception — `Event::PlayerState` arrives on its own, because
//! polling is a read and reading is free (COMPAT rule 3).

use std::time::Instant;

use crate::art;
use crate::config::{ArtProtocol, VisualizerSource};
use crate::player::actions::{CommandOutcome, PlayerCommand};
use crate::player::{PlaybackState, PlayerState, RepeatMode, TrackInfo};
use crate::tui::theme::{Accent, Border};
use crate::visualizer::AudioSource;
use crate::web::api::{Album, Artist, Page, Playlist, Queue, SearchResults, Track, TrackItem};

/// A track played this session. Session-only, cleared on exit (SPEC §2).
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub track: TrackInfo,
    pub at: Instant,
}

pub const HISTORY_CAP: usize = 500;

/// How many history rows the History tab will ever draw.
///
/// The list holds up to `HISTORY_CAP` entries, and drawing 500 of them into a
/// 20-row pane is wasted work on every frame. This also has to be the bound the
/// selection is clamped to, or the cursor can point at a row that is not there.
pub const HISTORY_VIEW: usize = 200;

/// How long a toast stays up (TODO 4.8).
const TOAST_SECS: f64 = 2.5;

/// The album art for the current track, and where it got to.
///
/// The download happens on a worker (TODO 4.1: never block the UI thread), so
/// this is a small state machine: nothing, asked for, on its way, here, or
/// failed. A failure is a placeholder, not a crash: the art is the first thing to
/// go when something is wrong, and the dashboard must survive losing it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArtState {
    /// The URL of the track now playing, or of the one being fetched.
    pub url: Option<String>,
    /// The fetched and decoded cover, ready to draw.
    pub loaded: Option<crate::player::actions::LoadedArt>,
    /// A fetch is in flight for `url`.
    pub loading: bool,
    /// Why the last fetch did not produce an image. One line, shown once.
    pub error: Option<String>,
}

impl ArtState {
    /// The image to draw, if it is the one for the track that is playing.
    ///
    /// Matching on the URL is what stops the wrong cover flashing up while a new
    /// one downloads: the old file is still on disk, and drawing it under a new
    /// title is worse than drawing nothing.
    pub fn drawable(&self, track: &TrackInfo) -> Option<&crate::player::actions::LoadedArt> {
        let want = track.artwork_url.as_deref();
        match (&self.url, &self.loaded, want) {
            (Some(have), Some(art), Some(want)) if have == want => Some(art),
            _ => None,
        }
    }

    /// Mark a fetch as started for `url`. Returns false if one is already
    /// running, which is what keeps a skip from queueing a download per poll.
    pub fn begin(&mut self, url: &str) -> bool {
        if self.loading {
            return false;
        }
        self.url = Some(url.to_string());
        self.loading = true;
        self.error = None;
        true
    }

    /// Give up a claimed slot without a result coming back, because the job was
    /// never accepted in the first place.
    pub fn abandon(&mut self) {
        self.loading = false;
        self.url = None;
    }

    /// True when the art for the current track still has to be fetched, or is
    /// being fetched. The loop asks this after every read.
    pub fn wants(&self, track: &TrackInfo) -> bool {
        let Some(want) = track.artwork_url.as_deref() else {
            return false;
        };
        if self.loading {
            return false;
        }
        match &self.url {
            Some(have) if have == want => self.loaded.is_none(),
            _ => true,
        }
    }
}

/// Which list a [`Event::Page`] is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageWhat {
    Playlists,
    Liked,
    LibraryAlbums,
    LibraryArtists,
    LibraryRecent,
    PlaylistItems(String),
    ArtistAlbums(String),
    AlbumTracks(String),
}

impl PageWhat {
    /// The tab this page belongs to, so an event knows where to put itself.
    pub fn tab(self) -> Tab {
        match self {
            PageWhat::Playlists => Tab::Playlists,
            PageWhat::Liked => Tab::Liked,
            PageWhat::LibraryAlbums | PageWhat::LibraryArtists | PageWhat::LibraryRecent => {
                Tab::Library
            }
            PageWhat::PlaylistItems(_) | PageWhat::ArtistAlbums(_) | PageWhat::AlbumTracks(_) => {
                // A page that was opened from a list stays on that list, so `1`
                // takes the user back to a tab rather than nowhere.
                Tab::Playlists
            }
        }
    }
}

/// A loaded page, still tagged with what asked for it: a list tab that has been
/// left and come back to must not have its cursor moved by a page the user is
/// no longer looking at.
#[derive(Debug, Clone, PartialEq)]
pub enum PageLoaded {
    Playlists(Page<Playlist>),
    Liked(Page<TrackItem>),
    LibraryAlbums(Page<Album>),
    LibraryArtists(Page<Artist>),
    LibraryRecent(Page<Track>),
    PlaylistItems { id: String, page: Page<TrackItem> },
    ArtistAlbums { id: String, page: Page<Album> },
    AlbumTracks { id: String, page: Page<Track> },
}

/// Everything the loop can tell the app.
#[derive(Debug, Clone)]
pub enum Event {
    Key(char),
    Resize,
    /// A poll came back. A read: never carries a write with it.
    PlayerState(Box<PlayerState>),
    /// A write finished. `Err` becomes a one-line notice, never a crash, and a
    /// read-back that says Spotify ignored the write hides the meter (COMPAT
    /// rule 5).
    CommandDone(CommandOutcome),
    /// Spotify is not running.
    NotRunning,
    /// Local tick, for interpolating the bar and expiring toasts.
    Tick,
    /// A frame tick, at the visualizer's frame rate and only while it is on
    /// screen (TODO 8.4/8.5). Separate from [`Event::Tick`] because the clock
    /// ticks once a second and a 30 fps bar needs thirty.
    VizTick,
    /// A search finished. `for_query` is what was asked, so a response for a
    /// question the user has already typed past is dropped rather than shown
    /// (TODO 7.6).
    Searched {
        for_query: String,
        result: Result<SearchResults, crate::web::api::ApiError>,
    },
    /// One of the list tabs loaded a page. Which one is a tag rather than five
    /// near-identical variants, because the handling is identical and five
    /// variants is five places to forget one.
    Page {
        what: PageWhat,
        result: Result<PageLoaded, crate::web::api::ApiError>,
    },
    /// The queue loaded (7.8).
    Queue(Result<Queue, crate::web::api::ApiError>),
    /// A write to the library landed or did not (7.7's `f`, 7.8's `A`).
    WebWrote(Result<(), crate::web::api::ApiError>),
    /// The connection state changed, from the token store or from a login.
    Connection(Connection),
    /// A lyrics lookup finished.
    Lyrics {
        uri: Option<String>,
        result: Result<crate::lyrics::Lyrics, crate::lyrics::LyricsError>,
    },
    /// Sonar's state file was re-read (TODO 4.6).
    Sonar(crate::sonar::SonarState),
    /// headless-spotify answered, once (TODO 4.7).
    Headless(crate::headless::Headless),
    /// A notification for the track that has just started (TODO 4.5). This is a
    /// *change*, never the first read of a session: starting trak must not
    /// announce whatever happened to be playing.
    SoundForTrackChanged,
    /// An image finished downloading and decoding, or failed to.
    Art {
        url: String,
        result: Result<crate::player::actions::LoadedArt, art::ArtError>,
    },
    /// A click, a drag or a wheel, already resolved to what was hit.
    ///
    /// The loop hit-tests against the regions the last frame recorded, so a click
    /// lands where the pixels are rather than where a second guess at the layout
    /// says they are.
    Mouse(Mouse),
    /// How many history rows fit on screen. The loop sends this whenever it
    /// changes, because only it knows the pane's height.
    Viewport(usize),
    Quit,
}

/// What a mouse event did, as far as the app is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    /// A press that is being held. Only meaningful for a drag, e.g. scrubbing
    /// the progress bar.
    Drag,
    Release,
    ScrollUp,
    ScrollDown,
}

/// A mouse event with its target already resolved by the renderer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mouse {
    pub action: MouseAction,
    pub target: Hit,
}

/// A transport control drawn in the Now Playing pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Prev,
    Toggle,
    Next,
}

/// Something on screen that can be clicked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    /// One of the tab labels.
    Tab(usize),
    /// A row of the history list, counting from the top of what is *shown*.
    HistoryRow(usize),
    /// Anywhere in the history list, for a wheel that is not over a row.
    HistoryPane,
    /// The progress bar, as a fraction of the track.
    Seek(f64),
    Control(Control),
}

/// Which pane the right side is showing.
///
/// Version A's five tabs exist whether or not a Client ID is configured: an empty
/// tab that says "run `trak config`" is more use than a tab that is not there,
/// because a person who does not know the feature exists cannot go looking for it
/// (TODO 7.13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Search,
    Playlists,
    Queue,
    Liked,
    Library,
    Lyrics,
    History,
    Info,
}

impl Tab {
    /// In the order the numbers go, so `[4]` is the fourth thing drawn and the
    /// key that selects it are the same fact stated once (TODO 7.13).
    pub const ALL: [Tab; 8] = [
        Tab::Search,
        Tab::Playlists,
        Tab::Queue,
        Tab::Liked,
        Tab::Library,
        Tab::Lyrics,
        Tab::History,
        Tab::Info,
    ];

    /// The tabs a Version B build has, which is what a Client ID unlocks the rest
    /// of. Kept separate from `ALL` so the B-mode hint can name them.
    pub const VERSION_A: [Tab; 5] = [
        Tab::Search,
        Tab::Playlists,
        Tab::Queue,
        Tab::Liked,
        Tab::Library,
    ];

    /// The digit that selects this tab, or `None` when it has none. The first six
    /// are numbered because they are the tabs a person navigates between; History
    /// and Info are reachable by `Tab` and are deliberately not numbered.
    pub fn digit(self) -> Option<char> {
        let i = Tab::ALL.iter().position(|t| *t == self)?;
        (i < 6).then(|| char::from(b'1' + i as u8))
    }

    pub fn from_digit(c: char) -> Option<Self> {
        let n = c.to_digit(10)? as usize;
        // The digits are 1-based -- a person counting tabs starts at one -- and
        // `ALL` is 0-based, so this is the one place that conversion happens.
        (1..=6).contains(&n).then(|| Tab::ALL[n - 1])
    }

    /// Whether this tab needs the Web API. A Client ID unlocks these and nothing
    /// else: History, Info and Lyrics come from AppleScript and LRCLIB.
    pub fn needs_web(self) -> bool {
        Tab::VERSION_A.contains(&self)
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Search => "Search",
            Tab::Playlists => "Playlists",
            Tab::Queue => "Queue",
            Tab::Liked => "Liked",
            Tab::Library => "Library",
            Tab::Lyrics => "Lyrics",
            Tab::History => "History",
            Tab::Info => "Info",
        }
    }

    pub fn next(self) -> Self {
        let i = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Tab::ALL[(i + 1) % Tab::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let i = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Tab::ALL[(i + Tab::ALL.len() - 1) % Tab::ALL.len()]
    }
}

/// Which volume trak changes (TODO 4.4, R2).
/// Everything the Web API tabs hold (TODO 7.6-7.11).
///
/// The pages are held as rows rather than as paging cursors, because a TUI list
/// that drops the first page when the user scrolls past the end is worse than one
/// that asks for more. `next` is kept so the list can go and get the rest when
/// the user reaches the bottom, and `None` is the end rather than "not asked".
#[derive(Debug, Clone)]
pub struct WebState {
    /// The connection, as the token store reports it. `NotConnected` is a state,
    /// not an error: it is what a person without a Client ID sees, and it is the
    /// thing the tabs explain rather than a red toast.
    pub connection: Connection,
    /// True once the user has been told about the Client ID, so the hint is not
    /// repeated on every launch of a build that will never have one.
    pub hint_shown: bool,

    // -- Search (7.6)
    /// Whether `/` has focused the input. While it is focused, every printable
    /// key is a character rather than a command.
    pub search_focus: bool,
    pub query: String,
    /// The query whose results are on screen. A response for anything else is
    /// dropped rather than shown, because a slow request for "ma" landing after
    /// a fast one for "massive attack" would replace the answer with the wrong
    /// question.
    pub search_shown: String,
    pub results: SearchResults,
    /// Which of the four result groups the cursor is in, moved with `Tab`.
    pub group: usize,
    pub group_row: [usize; 4],
    pub searching: bool,
    /// Seconds until the next debounced search is allowed to fire. The loop owns
    /// the clock; the state owns the decision.
    pub search_debounce: f64,

    // -- The list tabs (7.7-7.9)
    pub playlists: Page<Playlist>,
    pub playlist_cursor: usize,
    pub liked: Page<TrackItem>,
    /// One cursor per list, not one for the tab. A single shared cursor across
    /// three lists is a cursor that points at a row of whichever list happens to
    /// be longer, which is not a selection.
    pub liked_cursor: usize,
    pub library_cursor: usize,
    pub queue_cursor: usize,
    pub library: LibrarySections,
    pub queue: Queue,
    pub liked_here: Option<bool>,

    // -- The pages you open something into (7.10)
    /// A navigation stack, so `esc` from an album inside an artist returns to the
    /// artist rather than to the tab. An artist page that cannot be left the way
    /// it was entered is a trap.
    pub pages: Vec<Page_>,
    /// What the list tab is showing, when a page is open it hides the list.
    pub open: Option<Open>,
    /// The rows of the open page, kept in whichever shape they arrived in so the
    /// renderer does not have to know which kind of page this is. The `open` tag
    /// is the answer to "which", so there is only ever one list on screen.
    pub open_tracks: Vec<TrackItem>,
    pub open_albums: Vec<Album>,
    pub open_track_page: Vec<Track>,
    pub open_cursor: usize,
}

impl WebState {
    /// Show a playlist's tracklist (7.7).
    pub fn open_playlist(&mut self, id: String) {
        self.pages.push(Page_::Playlist(id.clone()));
        self.open = Some(Open::Playlist(id));
        self.open_tracks.clear();
        self.open_albums.clear();
        self.open_track_page.clear();
        self.open_cursor = 0;
    }

    /// Show an artist's albums (7.10). Albums only, and the reason is in
    /// `docs/WEB-API.md`: top-tracks is gone.
    pub fn open_artist(&mut self, id: String) {
        self.pages.push(Page_::Artist(id.clone()));
        self.open = Some(Open::Artist(id));
        self.open_tracks.clear();
        self.open_albums.clear();
        self.open_track_page.clear();
        self.open_cursor = 0;
    }

    /// Show an album's tracklist (7.10).
    pub fn open_album(&mut self, id: String) {
        self.pages.push(Page_::Album(id.clone()));
        self.open = Some(Open::Album(id));
        self.open_tracks.clear();
        self.open_albums.clear();
        self.open_track_page.clear();
        self.open_cursor = 0;
    }

    pub fn playlist_items(&mut self, id: String, items: Vec<TrackItem>) {
        let _ = id;
        self.open_tracks = items;
        self.open_cursor = 0;
    }

    pub fn artist_albums(&mut self, id: String, items: Vec<Album>) {
        let _ = id;
        self.open_albums = items;
        self.open_cursor = 0;
    }

    pub fn album_tracks(&mut self, id: String, items: Vec<Track>) {
        let _ = id;
        self.open_track_page = items;
        self.open_cursor = 0;
    }

    /// Leave one level. `false` when there was nothing to leave, which is the
    /// caller's cue to close the overlay or leave the tab.
    pub fn close_page(&mut self) -> bool {
        self.pages.pop();
        self.open = self.pages.last().map(|page| match page {
            Page_::Playlist(id) => Open::Playlist(id.clone()),
            Page_::Artist(id) => Open::Artist(id.clone()),
            Page_::Album(id) => Open::Album(id.clone()),
        });
        self.open_cursor = 0;
        !self.pages.is_empty()
    }

    /// The rows of whatever is open, as the display strings for it. One list so
    /// the renderer has one thing to draw and the key handling has one cursor.
    pub fn open_rows(&self) -> Vec<String> {
        match &self.open {
            Some(Open::Playlist(_)) => self
                .open_tracks
                .iter()
                .filter_map(|item| item.track.as_ref())
                .map(|track| track.to_string())
                .collect(),
            Some(Open::Artist(_)) => self.open_albums.iter().map(|a| a.to_string()).collect(),
            Some(Open::Album(_)) => self.open_track_page.iter().map(|t| t.to_string()).collect(),
            None => Vec::new(),
        }
    }

    /// The URI of the row the cursor is on, for `enter`. `None` for a row that is
    /// not a track -- an album row is opened with `enter`, not played.
    pub fn open_row_uri(&self) -> Option<String> {
        match self.open.as_ref()? {
            Open::Playlist(_) => self
                .open_tracks
                .get(self.open_cursor)
                .and_then(|i| i.track.as_ref())
                .map(|t| t.uri.clone()),
            Open::Album(_) => self
                .open_track_page
                .get(self.open_cursor)
                .map(|t| t.uri.clone()),
            // An artist page lists albums, and an album is not playable by URI.
            Open::Artist(_) => None,
        }
    }
}

impl Default for WebState {
    fn default() -> Self {
        Self {
            connection: Connection::default(),
            hint_shown: false,
            search_focus: false,
            query: String::new(),
            search_shown: String::new(),
            results: SearchResults::default(),
            group: 0,
            group_row: [0; 4],
            searching: false,
            search_debounce: 0.0,
            playlists: Page::empty(),
            playlist_cursor: 0,
            liked: Page::empty(),
            liked_cursor: 0,
            library_cursor: 0,
            queue_cursor: 0,
            library: LibrarySections::default(),
            queue: Queue::default(),
            liked_here: None,
            pages: Vec::new(),
            open: None,
            open_tracks: Vec::new(),
            open_albums: Vec::new(),
            open_track_page: Vec::new(),
            open_cursor: 0,
        }
    }
}

impl Default for LibrarySections {
    fn default() -> Self {
        Self {
            section: LibrarySection::Albums,
            albums: Page::empty(),
            artists: Page::empty(),
            recent: Page::empty(),
            loaded: [false; 3],
        }
    }
}

/// Which list the Library tab is showing (7.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibrarySection {
    Albums,
    Artists,
    Recent,
}

impl LibrarySection {
    pub const ALL: [LibrarySection; 3] = [
        LibrarySection::Albums,
        LibrarySection::Artists,
        LibrarySection::Recent,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Albums => "Saved albums",
            Self::Artists => "Followed artists",
            Self::Recent => "Recently played",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// The three lists of the Library tab, loaded lazily (7.9).
#[derive(Debug, Clone)]
pub struct LibrarySections {
    pub section: LibrarySection,
    pub albums: Page<Album>,
    pub artists: Page<Artist>,
    pub recent: Page<Track>,
    /// Which of the three have been asked for. Loading all three on entry would
    /// be three requests for a tab most visits leave immediately.
    pub loaded: [bool; 3],
}

/// What a list tab has opened, or nothing.
#[derive(Debug, Clone, PartialEq)]
pub enum Open {
    /// A playlist's tracklist (7.7).
    Playlist(String),
    /// An artist's albums (7.10). Albums only: `top-tracks` was removed in dev
    /// mode with no replacement, and the docs say so.
    Artist(String),
    /// An album's tracklist (7.10).
    Album(String),
}

/// One entry on the navigation stack.
#[derive(Debug, Clone, PartialEq)]
pub enum Page_ {
    Playlist(String),
    Artist(String),
    Album(String),
}

impl Page_ {
    pub fn title(&self) -> &'static str {
        match self {
            Page_::Playlist(_) => "Playlist",
            Page_::Artist(_) => "Artist",
            Page_::Album(_) => "Album",
        }
    }
}

/// How long a keystroke waits before a search goes out (TODO 7.6).
///
/// 250 ms is long enough that typing "massive attack" is one request rather than
/// thirteen, and short enough that the list feels attached to the keyboard.
pub const SEARCH_DEBOUNCE_SECS: f64 = 0.25;

/// Whether trak has a usable Spotify connection (7.2, 7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Connection {
    /// No Client ID in the config, so no login has been possible.
    #[default]
    NoClientId,
    /// A Client ID but no token file: the user has to run the guided setup.
    LoggedOut,
    Connected,
    /// The refresh token is spent or nearly so. A state to say out loud, not an
    /// error to sit in.
    NeedsRelogin,
}

impl Connection {
    pub fn connected(self) -> bool {
        self == Connection::Connected
    }

    /// The one line that says what to do about it. `None` when there is nothing
    /// to say, which is the case that matters most: a connected client should
    /// not be talking about connecting.
    pub fn notice(self) -> Option<&'static str> {
        match self {
            Connection::Connected => None,
            Connection::NoClientId => {
                Some("add a Spotify Client ID in `trak config` for search and playlists")
            }
            Connection::LoggedOut => Some("connect Spotify in `trak config`"),
            Connection::NeedsRelogin => Some("reconnect Spotify in `trak config`"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeControl {
    /// Spotify's own volume, which is what a music player should change.
    Spotify,
    /// The **system** output volume, for when Spotify ignores AppleScript volume
    /// sets. A fallback, not a preference: it turns down every sound on the
    /// machine, so the setting exists for the user to choose it deliberately.
    System,
}

impl VolumeControl {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "spotify" => Some(Self::Spotify),
            "system" => Some(Self::System),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Spotify => "spotify",
            Self::System => "system",
        }
    }
}

/// Where a lyrics lookup has got to. Four states rather than an `Option`,
/// because "looked and found nothing" and "never looked" are different answers
/// and the tab has to say which one it is showing.
#[derive(Debug, Clone, PartialEq)]
pub enum LyricsStatus {
    /// Nothing looked up yet for this track.
    Idle,
    /// A lookup is in flight.
    Loading,
    /// Lines are here. May be synced or not, and may be zero lines for an
    /// instrumental.
    Ready,
    /// LRCLIB has nothing for this track. Common, and not an error.
    NotFound,
    /// The lookup failed for some other reason. One line explains it.
    Failed(String),
}

/// The lyrics for the current track.
#[derive(Debug, Clone, PartialEq)]
pub struct LyricsState {
    pub status: LyricsStatus,
    pub lyrics: Option<crate::lyrics::Lyrics>,
    /// The URI the lyrics belong to, so a lookup for a track the user has left is
    /// dropped rather than shown under a new title.
    pub uri: Option<String>,
}

impl Default for LyricsState {
    fn default() -> Self {
        Self {
            status: LyricsStatus::Idle,
            lyrics: None,
            uri: None,
        }
    }
}

impl LyricsState {
    /// The index of the line to show at this playback position, or `None` when
    /// there is nothing to show.
    pub fn active(&self, position_secs: f64) -> Option<usize> {
        crate::lyrics::index_at(self.lyrics.as_ref()?, position_secs)
    }
}

/// A transient message, bottom right (TODO 4.8).
#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub at: Instant,
}

/// What fills the art pane (SPEC §8 `[display] mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    /// The cover.
    Art,
    /// The visualizer. The cover is still fetched and its colour still drives the
    /// accent -- a visualizer tinted by the album is the whole point.
    Visualizer,
}

impl DisplayMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "art" => Some(Self::Art),
            "visualizer" => Some(Self::Visualizer),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Art => Self::Visualizer,
            Self::Visualizer => Self::Art,
        }
    }
}

/// The visualizer styles SPEC §8 offers (TODO 8.1 draws them; 4.3 only carries
/// the choice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualizerStyle {
    Spectrum,
    Mirrored,
    Waveform,
    Circular,
}

impl VisualizerStyle {
    pub const ALL: [VisualizerStyle; 4] = [
        VisualizerStyle::Spectrum,
        VisualizerStyle::Mirrored,
        VisualizerStyle::Waveform,
        VisualizerStyle::Circular,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "spectrum" => Some(Self::Spectrum),
            "mirrored" => Some(Self::Mirrored),
            "waveform" => Some(Self::Waveform),
            "circular" => Some(Self::Circular),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Spectrum => "spectrum",
            Self::Mirrored => "mirrored",
            Self::Waveform => "waveform",
            Self::Circular => "circular",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// One field of SPEC §8, and everything the renderer needs to know about it.
/// Every key in the config file has a field here, so that wiring a setting to
/// behaviour is a matter of reading a field rather than of threading a new
/// argument through the renderer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    // [input]
    pub seek_step: f64,
    pub volume_step: i16,
    /// On by default. When off the loop does not even ask the terminal for mouse
    /// events, so a terminal that reports them cannot steal text selection.
    pub mouse: bool,
    // [display]
    /// Whether to fetch and draw the cover at all. Ignored while the mode is the
    /// visualizer, which draws over the same rectangle.
    pub show_art: bool,
    pub show_clock: bool,
    pub show_volume: bool,
    pub show_key_hints: bool,
    pub side_pane: bool,
    /// The progress bar under the title block. Off by default only because SPEC
    /// §8 says so; the bar is the main way to see how far in a track you are.
    pub show_progress: bool,
    /// The `▰▱` popularity row on the Info tab.
    pub show_popularity: bool,
    /// Which tab opens on launch.
    pub default_tab: Tab,
    /// Rounded by default (SPEC §2). Held as the enum rather than as the
    /// `rounded` bool it used to be, because `double` and `none` are in the file
    /// too and a bool cannot say which of them was meant.
    pub border: Border,
    pub accent: Accent,
    pub art_protocol: ArtProtocol,
    /// `[display] mode`. `a` toggles it.
    pub display_mode: DisplayMode,
    // [visualizer]
    pub visualizer_style: VisualizerStyle,
    pub visualizer_source: VisualizerSource,
    // [lyrics]
    pub lyrics: bool,
    // [notifications]
    pub song_change_notification: bool,
    // [volume]
    pub volume_control: VolumeControl,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            seek_step: 5.0,
            volume_step: 10,
            mouse: true,
            show_art: true,
            show_clock: true,
            show_volume: true,
            show_key_hints: true,
            side_pane: true,
            show_progress: true,
            show_popularity: true,
            default_tab: Tab::History,
            border: Border::Rounded,
            accent: Accent::Art,
            art_protocol: ArtProtocol::Auto,
            display_mode: DisplayMode::Art,
            visualizer_style: VisualizerStyle::Spectrum,
            visualizer_source: VisualizerSource::Auto,
            lyrics: true,
            song_change_notification: false,
            volume_control: VolumeControl::Spotify,
        }
    }
}

impl Settings {
    /// Rounded corners, the SPEC §2 default. Kept as a question rather than a
    /// field so a caller asking "is it rounded" cannot be answered with a bool
    /// that has quietly lost the difference between `double` and `none`.
    pub fn rounded(&self) -> bool {
        self.border == Border::Rounded
    }
}

#[derive(Debug, Clone)]
pub struct App {
    /// `None` means Spotify is not running and the idle card is up.
    pub state: Option<PlayerState>,
    pub history: Vec<HistoryEntry>,
    pub tab: Tab,
    pub history_cursor: usize,
    /// The first history row shown. The list can be 500 long and the pane 20
    /// rows high, so the view has to follow the cursor or `j` walks it off the
    /// screen with nothing to show for it.
    pub history_scroll: usize,
    /// How many history rows fit. Sent by the loop, which is the only thing that
    /// knows the pane's height.
    pub viewport: usize,
    /// Whether the user has actually moved the selection. Without this, every
    /// track change would drag the cursor down with the new row, and a session
    /// left alone would end up selecting the *oldest* track.
    pub cursor_moved: bool,
    pub show_help: bool,
    /// What Spotify last reported, kept only so the meter has something to show
    /// before the user has chosen a volume of their own.
    pub read_volume: u8,
    /// The volume the user last chose, which is what the meter shows once they
    /// have. A poll never overwrites it: Spotify's read-back is quantised and
    /// would make the meter jitter by 1 % (COMPAT rule 5), and during a Sonar
    /// fade the read *is* the mid-fade value trak must not present as the user's
    /// (COMPAT rule 3).
    pub user_volume: Option<u8>,
    /// Set when a volume write did not land. The meter is then a lie, so it is
    /// hidden rather than shown wrong (COMPAT rule 5).
    pub volume_hidden: bool,
    pub muted: bool,
    pub pre_mute_volume: u8,
    pub repeat: RepeatMode,
    /// When the last read landed, so the bar can interpolate between reads.
    pub last_read: Option<Instant>,
    /// A command in flight, so keys are not double-applied.
    pub busy: Option<PlayerCommand>,
    pub toast: Option<Toast>,
    pub settings: Settings,
    /// The loaded `config.toml`, kept whole rather than as its settings because
    /// it carries keys this build does not know about yet. Saving starts from
    /// here, so a key added by a newer trak, or by hand, survives a save from
    /// this one instead of being quietly deleted.
    pub config: crate::config::Config,
    /// Set when a setting has changed and the file has not been written yet.
    pub config_dirty: bool,
    /// The first-run hint, shown until the first key (TODO 5.4). `None` once it
    /// has been dismissed, and it is never saved: the config file appearing at
    /// all is what makes the next launch a *not*-first run.
    pub hint: Option<String>,
    /// The settings overlay is up (TODO 5.2). Key handling goes to the screen
    /// while it is, so a stray `q` cannot quit trak from behind a dialog.
    pub settings_open: bool,
    /// Which row of the settings overlay has the cursor (TODO 5.2).
    pub settings_cursor: usize,
    /// The visualizer's bars and where they come from (TODO 8.4). The source is
    /// seeded from the track so a new song visibly looks different, and the
    /// spectrum is kept on the app because the render path must not block.
    pub viz: crate::visualizer::SimulatedSource,
    pub spectrum: Vec<f32>,
    /// TODO 4.1: the album art and its fetch state.
    pub art: ArtState,
    /// The lyrics tab (TODO 6.1).
    pub lyrics: LyricsState,
    /// The Web API tabs (TODO 7.6-7.11).
    pub web: WebState,
    /// What Sonar is doing right now (TODO 4.6).
    pub sonar: crate::sonar::SonarState,
    /// What headless-spotify is doing right now (TODO 4.7).
    pub headless: crate::headless::Headless,
    pub should_quit: bool,
    /// Local clock, for the header.
    pub clock: String,
    /// How far the title has scrolled, so a long title is readable rather than
    /// truncated. Reset when the track changes.
    pub marquee_offset: usize,
    /// Seconds since the app started, used by the breathing status dot. Ticked
    /// locally so the renderer needs no clock of its own.
    pub tick_secs: f64,
    /// Whether a poll is due. Ticked locally so the loop stays testable.
    pub poll_due: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            state: None,
            history: Vec::new(),
            tab: Settings::default().default_tab,
            history_cursor: 0,
            history_scroll: 0,
            viewport: 10,
            cursor_moved: false,
            show_help: false,
            read_volume: 0,
            user_volume: None,
            volume_hidden: false,
            muted: false,
            pre_mute_volume: 0,
            repeat: RepeatMode::Off,
            last_read: None,
            busy: None,
            toast: None,
            settings: Settings::default(),
            config: crate::config::Config::default(),
            config_dirty: false,
            hint: None,
            settings_open: false,
            settings_cursor: 0,
            // Seeded from the clock rather than a constant so two trak processes
            // do not draw in lockstep, and so a snapshot test can pin it.
            viz: crate::visualizer::SimulatedSource::new(0),
            spectrum: vec![0.0; crate::visualizer::BARS],
            art: ArtState::default(),
            // What Sonar is doing right now (TODO 4.6).
            sonar: crate::sonar::SonarState::unknown(),
            // What headless-spotify is doing right now (TODO 4.7).
            headless: crate::headless::Headless::unknown(),
            lyrics: LyricsState::default(),
            web: WebState::default(),
            should_quit: false,
            clock: String::new(),
            marquee_offset: 0,
            tick_secs: 0.0,
            poll_due: true,
        }
    }

    pub fn is_playing(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|s| s.playback == PlaybackState::Playing)
    }

    pub fn track(&self) -> Option<&TrackInfo> {
        self.state.as_ref().map(|s| &s.track)
    }

    /// The volume to draw: what the user chose, or Spotify's own value until
    /// they choose one.
    pub fn meter_volume(&self) -> u8 {
        self.user_volume.unwrap_or(self.read_volume)
    }

    /// Spotify is not running.
    pub fn is_idle(&self) -> bool {
        self.state.is_none()
    }

    /// The position to draw, interpolated since the last read.
    ///
    /// A read is ~430 ms and the poll is seconds, so interpolating locally is the
    /// only way the bar moves smoothly (ARCHITECTURE, "Progress bar is
    /// interpolated locally"). It must not run past the end, and it must not
    /// advance while paused.
    pub fn interpolated_position(&self) -> f64 {
        let Some(s) = &self.state else { return 0.0 };
        if s.playback != PlaybackState::Playing {
            return s.position_secs;
        }
        let Some(last) = self.last_read else {
            return s.position_secs;
        };
        let dur = s.track.duration_secs() as f64;
        let advanced = s.position_secs + last.elapsed().as_secs_f64();
        if dur > 0.0 {
            advanced.min(dur)
        } else {
            advanced
        }
    }

    /// Move the selection to `row` in the view, scrolling to keep it on screen.
    ///
    /// Every way the cursor moves goes through here. Doing it in each key handler
    /// is how the scroll and the cursor drift apart, which shows up as a
    /// selection that has moved but is not on screen.
    fn select(&mut self, row: usize) {
        self.history_cursor = row.min(self.history_cursor_max());
        self.cursor_moved = true;
        self.scroll_to_cursor();
    }

    fn scroll_to_cursor(&mut self) {
        let page = self.viewport.max(1);
        if self.history_cursor < self.history_scroll {
            self.history_scroll = self.history_cursor;
        } else if self.history_cursor >= self.history_scroll + page {
            self.history_scroll = self.history_cursor + 1 - page;
        }
        // Never scroll past the end: the last page should be full of history
        // rather than padded with blank rows.
        let last = self.history.len().saturating_sub(page);
        self.history_scroll = self.history_scroll.min(last);
    }

    /// The history rows as the tab shows them: newest first, from the scroll.
    pub fn visible_history(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.history.iter().rev().skip(self.history_scroll)
    }

    /// The row `enter` would play, or `None` when there is nothing to play.
    ///
    /// The cursor counts rows from the **newest**, because that is the order the
    /// tab draws them in. It is an index into the view, not into `history`, so
    /// the two are related by `len - 1 - i` — getting that backwards plays the
    /// wrong song, which is the sort of bug that only shows up on a real library.
    pub fn selected_history(&self) -> Option<&HistoryEntry> {
        self.history.iter().rev().nth(self.history_cursor)
    }

    /// The highest cursor value: the last row of the list.
    ///
    /// Every row is reachable now that the view scrolls. This used to stop at
    /// `HISTORY_VIEW`, which quietly made the oldest 300 rows of a long session
    /// unreachable — and, worse, unreachable *invisibly*, because the view did
    /// not scroll then either.
    fn history_cursor_max(&self) -> usize {
        self.history.len().saturating_sub(1)
    }

    /// How many times the current track has come round this session.
    ///
    /// `None` for an advert, which has no URI to count by. The current track
    /// counts as one, so a track heard once reads `1` and not `0` (TODO 3.7).
    pub fn times_heard(&self) -> Option<u32> {
        let uri = self.track()?.uri.as_ref()?;
        let past = self
            .history
            .iter()
            .filter(|e| e.track.uri.as_ref() == Some(uri))
            .count();
        Some(past as u32 + 1)
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            at: Instant::now(),
        });
    }

    fn expire_toast(&mut self) {
        if let Some(t) = &self.toast
            && t.at.elapsed().as_secs_f64() > TOAST_SECS
        {
            self.toast = None;
        }
    }
}

/// Everything `update` can return: the new app and the commands to run.
pub struct Updated {
    pub app: App,
    pub commands: Vec<PlayerCommand>,
}

/// The one place the app's state changes.
///
/// Returns the commands the caller should run rather than performing them, which
/// is what keeps this function pure and testable.
pub fn update(mut app: App, event: Event) -> Updated {
    let mut commands = Vec::new();

    match event {
        Event::Quit => app.should_quit = true,

        Event::Resize => {
            // The loop tells us the size directly; nothing to compute here, but
            // the event exists so the renderer and the breakpoints stay in step.
        }

        Event::Tick => {
            app.tick_secs += 0.1;
            app.expire_toast();
            // A command in flight is still running. Do not stack a poll behind it
            // or the queue grows without bound.
            app.poll_due = app.busy.is_none();
        }

        Event::Searched { for_query, result } => {
            // An answer to a question the user has already typed past is worse
            // than no answer: it puts results on screen that do not match the
            // box they came from.
            if for_query != app.web.query {
                return Updated { app, commands };
            }
            app.web.searching = false;
            app.web.search_shown = for_query;
            app.web.group_row = [0; 4];
            match result {
                Ok(r) => {
                    app.web.results = r;
                }
                Err(e) => app.toast(e.notice()),
            }
        }

        Event::Page { what, result } => match result {
            Ok(loaded) => apply_page(&mut app, what, loaded),
            Err(e) => app.toast(e.notice()),
        },

        Event::Queue(result) => match result {
            Ok(q) => app.web.queue = q,
            Err(e) => app.toast(e.notice()),
        },

        Event::WebWrote(result) => {
            if let Err(e) = result {
                app.toast(e.notice());
            } else {
                // The write landed, so the check is stale: re-read it rather than
                // leaving the row saying the old thing.
                app.web.liked_here = None;
            }
        }

        Event::Connection(c) => {
            app.web.connection = c;
            if let Some(n) = c.notice() {
                app.web.hint_shown = true;
                app.toast(n);
            }
        }

        Event::VizTick => {
            app.viz.set_position(app.interpolated_position());
            app.spectrum = app.viz.spectrum();
        }

        Event::Key(c) => handle_key(&mut app, c, &mut commands),

        Event::PlayerState(s) => {
            apply_state(&mut app, *s);
            app.poll_due = false;
        }

        Event::Viewport(rows) => {
            if app.viewport != rows {
                app.viewport = rows;
                // A resize can leave the view scrolled past the end.
                app.scroll_to_cursor();
            }
        }

        Event::Lyrics { uri, result } => {
            // Lyrics for a track the user has already skipped past: drop them.
            if uri == app.track().and_then(|t| t.uri.clone()) {
                app.lyrics.status = match result {
                    Ok(l) => {
                        app.lyrics.lyrics = Some(l);
                        LyricsStatus::Ready
                    }
                    // "Not found" is the common case, not a failure: most tracks
                    // are not in LRCLIB.
                    Err(crate::lyrics::LyricsError::NotFound) => {
                        app.lyrics.lyrics = None;
                        LyricsStatus::NotFound
                    }
                    Err(e) => {
                        app.lyrics.lyrics = None;
                        LyricsStatus::Failed(e.notice())
                    }
                };
            }
        }

        Event::SoundForTrackChanged => {
            if app.settings.song_change_notification
                && let Some(track) = app.track()
            {
                let body = match (track.artist.is_empty(), track.album.is_empty()) {
                    (true, _) => String::new(),
                    (false, true) => track.artist.clone(),
                    _ => format!("{} — {}", track.artist, track.album),
                };
                commands.push(PlayerCommand::Notify(track.title.clone(), body));
            }
        }

        Event::Sonar(state) => {
            // A duck that has just begun is the one thing the user needs to know
            // about before reaching for the volume keys, which are refused while
            // it lasts.
            let was_ducking = app.sonar.is_ducking();
            app.sonar = state;
            if app.sonar.is_ducking() && !was_ducking {
                app.toast(app.sonar.notice());
            }
        }

        Event::Headless(h) => app.headless = h,

        Event::Art { url, result } => {
            // The slot is free whatever happened. Clearing this only on the happy
            // path is how one skipped track leaves art permanently disabled: a
            // result for a track we have left is dropped, and if it did not also
            // clear `loading` then `begin` would refuse every later request.
            app.art.loading = false;
            // A fetch for a track the user has already skipped past: drop it,
            // rather than showing a cover for the wrong song.
            let still_wanted = app
                .track()
                .and_then(|t| t.artwork_url.as_deref())
                .is_some_and(|want| want == url);
            if still_wanted {
                match result {
                    Ok(loaded) => {
                        app.art.loaded = Some(loaded);
                        app.art.error = None;
                    }
                    // One line, and the placeholder stays (TODO 4.1: art
                    // disappears cleanly; it must never take the dashboard with
                    // it).
                    Err(e) => {
                        app.art.loaded = None;
                        app.art.error = Some(e.notice());
                    }
                }
            }
        }

        Event::Mouse(m) => handle_mouse(&mut app, m, &mut commands),

        Event::NotRunning => {
            app.state = None;
            app.last_read = None;
            app.poll_due = false;
        }

        Event::CommandDone(outcome) => {
            app.busy = None;
            match outcome.result {
                Err(e) => {
                    // One line, never a stack trace (SPEC §6).
                    let (msg, _) = crate::cli::report(e);
                    app.toast(msg.lines().next().unwrap_or("error").to_string());
                }
                Ok(None) => {}
                Ok(Some(w)) => {
                    if w.landed {
                        // The write landed, so the meter is worth showing again.
                        app.volume_hidden = false;
                        if w.what == "volume" {
                            // Show what the player actually aimed for, which is
                            // the clamped value, rather than recomputing the
                            // clamp here and hoping the two agree.
                            app.user_volume = Some(w.wanted.clamp(0, 100) as u8);
                        }
                    } else {
                        if let Some(n) = w.notice() {
                            app.toast(n.lines().next().unwrap_or("write ignored").to_string());
                        }
                        // A volume meter that cannot be trusted is worse than no
                        // meter, and the notice is the only warning the user
                        // gets (COMPAT rule 5).
                        if w.what == "volume" {
                            app.volume_hidden = true;
                        }
                    }
                }
            }
        }
    }

    Updated { app, commands }
}

fn handle_key(app: &mut App, c: char, commands: &mut Vec<PlayerCommand>) {
    // The first-run hint goes on any key, before anything else reads it, so it
    // cannot swallow one.
    app.hint = None;

    if app.settings_open {
        crate::tui::settings::key(app, c);
        return;
    }

    if app.show_help {
        // The overlay swallows everything so a stray key cannot fire a write
        // behind it, but the quit keys still work.
        if matches!(c, 'q' | 'Q') {
            app.should_quit = true;
        } else {
            app.show_help = false;
        }
        return;
    }

    if app.is_idle() {
        // Idle card: enter is the only key that does anything, and it is the
        // single allowed launch (COMPAT rule 2).
        match c {
            '\n' | ' ' => commands.push(PlayerCommand::Launch),
            'q' | 'Q' => app.should_quit = true,
            _ => {}
        }
        return;
    }

    // A command is already running; queue nothing behind it.
    let busy = app.busy.is_some();

    match c {
        'q' | 'Q' => app.should_quit = true,
        '?' => app.show_help = true,
        // The art pane and the visualizer share one rectangle, so this swaps
        // what is drawn rather than what is laid out (TODO 4.3).
        'a' => {
            app.settings.display_mode = app.settings.display_mode.next();
            app.config_dirty = true;
        }
        'v' => {
            app.settings.visualizer_style = app.settings.visualizer_style.next();
            app.config_dirty = true;
        }
        ',' => {
            app.settings_open = true;
            crate::tui::settings::open(app);
        }
        '\t' => app.tab = app.tab.next(),
        // Shift-Tab arrives as an unbound sentinel from the event loop.
        'Z' => app.tab = app.tab.prev(),
        // Shift-Tab is a modifier key, not a char, and is handled in the loop.
        // One table, not one arm per digit: the number a tab is drawn with and
        // the key that selects it are the same fact, and two tables is one of
        // them being wrong.
        c if Tab::from_digit(c).is_some() => app.tab = Tab::from_digit(c).unwrap_or(app.tab),
        'j' => {
            let next = app.history_cursor + 1;
            app.select(next);
        }
        'k' => {
            app.select(app.history_cursor.saturating_sub(1));
        }
        // COMPAT rule 3: while Sonar is fading the volume, trak must not write a
        // volume at all. A relative step is computed from the *live* volume, which
        // mid-fade is Sonar's own value, so even pressing `-` would write a
        // mid-fade number back. The mute is named in the rule and is disabled
        // with the rest.
        // On the system control the volume keys are somebody else's volume, so
        // trak says so rather than silently doing nothing (TODO 4.4).
        _ if matches!(c, '+' | '=' | '-' | '_')
            && app.settings.volume_control == VolumeControl::System =>
        {
            app.toast("volume is on the system control, which changes every sound on this Mac");
        }
        _ if app.sonar.is_ducking() && matches!(c, 'm' | '+' | '=' | '-' | '_') => {
            app.toast("Sonar is adjusting the volume — leave it alone for a moment");
        }
        _ if busy => {}
        // `enter` plays the selected history row (SPEC §4). It only means that on
        // the History tab, because that is the only one with a selection; on the
        // other tabs it is a no-op rather than something that guesses.
        '\n' => {
            if app.tab == Tab::History
                && let Some(entry) = app.selected_history()
                && let Some(uri) = entry.track.uri.clone()
            {
                push(app, commands, PlayerCommand::PlayUri(uri));
            }
        }
        ' ' => push(app, commands, PlayerCommand::Toggle),
        'n' => push(app, commands, PlayerCommand::Next),
        'p' => push(app, commands, PlayerCommand::Prev),
        'r' => {
            // Advance the shown mode immediately rather than waiting for the
            // write to come back, so the key feels like it did something.
            app.repeat = app.repeat.next();
            push(app, commands, PlayerCommand::CycleRepeat);
        }
        's' => push(app, commands, PlayerCommand::ToggleShuffle),
        'R' => push(app, commands, PlayerCommand::Replay),
        'h' => push(app, commands, PlayerCommand::Seek(-app.settings.seek_step)),
        'l' => push(app, commands, PlayerCommand::Seek(app.settings.seek_step)),
        'm' => {
            app.muted = !app.muted;
            if app.muted {
                // What the user had, which may be their own choice or Spotify's
                // own value if they have not touched it yet.
                app.pre_mute_volume = app.meter_volume();
                app.user_volume = Some(0);
                push(app, commands, PlayerCommand::SetVolume(0));
            } else {
                app.user_volume = Some(app.pre_mute_volume);
                push(app, commands, PlayerCommand::SetVolume(app.pre_mute_volume));
            }
        }
        '+' | '=' => push(
            app,
            commands,
            PlayerCommand::VolumeStep(app.settings.volume_step),
        ),
        '-' | '_' => push(
            app,
            commands,
            PlayerCommand::VolumeStep(-app.settings.volume_step),
        ),
        'c' => {
            // Copy the share URL (TODO 3.11). The clipboard write goes to the
            // worker so a slow pasteboard cannot block the render loop.
            if let Some(uri) = app.track().and_then(|t| t.uri.clone()) {
                let id = uri.rsplit(':').next().unwrap_or_default();
                let link = format!("https://open.spotify.com/track/{id}");
                app.toast(format!("copied {link}"));
                if app.busy.is_none() {
                    app.busy = Some(PlayerCommand::CopyLink(link.clone()));
                    commands.push(PlayerCommand::CopyLink(link));
                }
            }
        }
        _ => {}
    }
}

/// How far a wheel notch moves the selection.
const WHEEL_LINES: usize = 3;

fn handle_mouse(app: &mut App, m: Mouse, commands: &mut Vec<PlayerCommand>) {
    // The loop does not ask for mouse events when this is off, but a terminal
    // that reports them anyway must not turn them into Spotify writes
    // (COMPAT rule 3).
    if !app.settings.mouse {
        return;
    }
    // The overlay is modal: a click behind it must not reach the dashboard, and a
    // click *on* it just closes it.
    if app.show_help {
        app.show_help = false;
        return;
    }
    // COMPAT rule 3 again: a click is a user action, but a *drag* is only a user
    // action while the button is held, and a release is not a command at all.
    if m.action == MouseAction::Release {
        return;
    }

    match m.target {
        Hit::Tab(i) => {
            if let Some(tab) = Tab::ALL.get(i) {
                app.tab = *tab;
            }
        }
        Hit::HistoryRow(i) => {
            if m.action == MouseAction::Press {
                app.select(app.history_scroll + i);
            }
        }
        Hit::HistoryPane => {
            let down = matches!(m.action, MouseAction::ScrollDown);
            let row = if down {
                app.history_cursor + WHEEL_LINES
            } else {
                app.history_cursor.saturating_sub(WHEEL_LINES)
            };
            app.select(row);
        }
        Hit::Seek(fraction) => {
            let dur = app.track().map(|t| t.duration_secs()).unwrap_or(0);
            if dur > 0 {
                push(
                    app,
                    commands,
                    PlayerCommand::Seek((fraction.clamp(0.0, 1.0) * dur as f64).round()),
                );
            }
        }
        Hit::Control(c) => {
            let cmd = match c {
                Control::Prev => PlayerCommand::Prev,
                Control::Toggle => PlayerCommand::Toggle,
                Control::Next => PlayerCommand::Next,
            };
            push(app, commands, cmd);
        }
    }
}

/// Queue a command unless one is already in flight.
fn push(app: &mut App, commands: &mut Vec<PlayerCommand>, cmd: PlayerCommand) {
    if app.busy.is_some() {
        return;
    }
    app.busy = Some(cmd.clone());
    commands.push(cmd);
}

/// Put a loaded page where it belongs.
///
/// A page for a tab the user is not on still lands, because switching tabs must
/// not have to go and fetch again -- but a page for a *page* that has since been
/// left is dropped, because opening an album and going back to the list must not
/// leave the list showing the album.
fn apply_page(app: &mut App, _what: PageWhat, loaded: PageLoaded) {
    // A page for a *page* that has since been left is dropped: opening an album
    // and going back to the list must not leave the list showing the album.
    // The list tabs are not checked, because switching tabs and coming back must
    // not have to fetch again.
    let open_now = app.web.open.clone();
    match loaded {
        PageLoaded::Playlists(p) => app.web.playlists = p,
        PageLoaded::Liked(p) => app.web.liked = p,
        PageLoaded::LibraryAlbums(p) => {
            app.web.library.albums = p;
            app.web.library.loaded[0] = true;
        }
        PageLoaded::LibraryArtists(p) => {
            app.web.library.artists = p;
            app.web.library.loaded[1] = true;
        }
        PageLoaded::LibraryRecent(p) => {
            app.web.library.recent = p;
            app.web.library.loaded[2] = true;
        }
        PageLoaded::PlaylistItems { id, page } => {
            if open_now == Some(Open::Playlist(id.clone())) {
                app.web.playlist_items(id, page.items);
            }
        }
        PageLoaded::ArtistAlbums { id, page } => {
            if open_now == Some(Open::Artist(id.clone())) {
                app.web.artist_albums(id, page.items);
            }
        }
        PageLoaded::AlbumTracks { id, page } => {
            if open_now == Some(Open::Album(id.clone())) {
                app.web.album_tracks(id, page.items);
            }
        }
    }
}

fn apply_state(app: &mut App, s: PlayerState) {
    let new_uri = s.track.uri.clone();
    let track_changed = app
        .track()
        .and_then(|t| t.uri.clone())
        .is_some_and(|uri| new_uri.as_deref() != Some(uri.as_str()));

    // Record the outgoing track, deduped against the one already recorded.
    if track_changed
        && let Some(prev) = app.track().cloned()
        && prev.uri.is_some()
    {
        app.history.push(HistoryEntry {
            track: prev,
            at: Instant::now(),
        });
        if app.history.len() > HISTORY_CAP {
            let excess = app.history.len() - HISTORY_CAP;
            app.history.drain(0..excess);
        }
        // The tab draws newest first, so the new row goes to the *front* of the
        // view and everything the user was looking at shifts down one. Stepping
        // the cursor keeps the same song selected instead of yanking the
        // selection onto whatever happens to be above it -- but only if the
        // selection is the user's to keep. A cursor nobody has touched stays at
        // the newest row, which is where it belongs.
        if app.cursor_moved {
            app.history_cursor += 1;
            app.scroll_to_cursor();
        }
    }

    // A poll updates only what Spotify is the authority for. The meter is the
    // user's, and stays theirs: `apply_state` must never write to it, or a Sonar
    // fade would show up as trak having moved the volume (COMPAT rule 3).
    app.read_volume = s.volume;
    app.repeat = if s.repeating_enabled {
        RepeatMode::Context
    } else {
        RepeatMode::Off
    };
    app.last_read = Some(Instant::now());
    // A new track's title starts at its beginning, and the last track's chorus
    // does not follow it: showing the wrong lyrics under a new title is worse
    // than showing none.
    app.marquee_offset = 0;
    app.lyrics = LyricsState::default();
    // The visualizer follows the music, so a new track gets a new shape and a
    // pause decays the bars rather than freezing them (TODO 8.4).
    if track_changed {
        app.viz.set_track(new_uri.as_deref().unwrap_or(""));
    }
    let playing = s.playback == PlaybackState::Playing;
    app.state = Some(s);
    app.viz.set_playing(playing);
    app.viz.set_position(app.interpolated_position());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::PlayerError;
    use crate::player::actions::{CommandOutcome, WriteOutcome};
    use crate::player::fake::sample_track;
    use crate::player::parse::parse;
    use crate::testutil::fixture;

    fn playing() -> PlayerState {
        parse(&fixture("playing_track.txt")).unwrap()
    }

    fn with_track() -> App {
        update(App::new(), Event::PlayerState(Box::new(playing()))).app
    }

    /// `update` returns a struct; tests want a pair, so this unwraps it.
    fn press(app: App, c: char) -> (App, Vec<PlayerCommand>) {
        let u = update(app, Event::Key(c));
        (u.app, u.commands)
    }

    fn step(app: App, e: Event) -> (App, Vec<PlayerCommand>) {
        let u = update(app, e);
        (u.app, u.commands)
    }

    #[test]
    fn a_first_read_records_no_history() {
        let app = with_track();
        assert!(app.history.is_empty(), "there is no previous track yet");
        assert_eq!(app.meter_volume(), 100);
        assert_eq!(app.user_volume, None, "the user has not chosen one yet");
        assert!(app.last_read.is_some());
    }

    /// The history replays by URI, and an advert has none, so it must never get in.
    #[test]
    fn an_advert_never_enters_history() {
        let ad = parse(&fixture("playing_ad.txt")).unwrap();
        let app = update(App::new(), Event::PlayerState(Box::new(ad))).app;
        assert!(app.history.is_empty());
        assert_eq!(app.track().unwrap().title, "Legal Services");
    }

    #[test]
    fn a_track_change_records_the_outgoing_track() {
        let app = with_track();
        let first = app.track().unwrap().clone();
        let mut second = playing();
        second.track.uri = Some("spotify:track:DIFFERENT".into());
        second.track.title = "Something Else".into();
        let (app, _) = press_state(app, second);

        assert_eq!(app.history.len(), 1);
        assert_eq!(app.history[0].track.title, first.title);
        assert_eq!(app.track().unwrap().title, "Something Else");
    }

    fn press_state(app: App, s: PlayerState) -> (App, Vec<PlayerCommand>) {
        let u = update(app, Event::PlayerState(Box::new(s)));
        (u.app, u.commands)
    }

    /// Re-reading the same track must not fill the history with one song.
    #[test]
    fn rereading_the_same_track_records_nothing() {
        let mut app = with_track();
        for _ in 0..5 {
            app = update(app, Event::PlayerState(Box::new(playing()))).app;
        }
        assert!(app.history.is_empty(), "{:?}", app.history);
    }

    #[test]
    fn history_is_capped_at_five_hundred() {
        let mut app = with_track();
        for n in 0..(HISTORY_CAP + 40) {
            let mut s = playing();
            s.track.uri = Some(format!("spotify:track:t{n}"));
            s.track.title = format!("Track {n}");
            app = update(app, Event::PlayerState(Box::new(s))).app;
        }
        assert_eq!(app.history.len(), HISTORY_CAP);
        // Each iteration records the *previous* track, so 540 iterations leave
        // [Census, Track 0 .. Track 538]. Dropping the oldest 40 leaves
        // Track 39 first and Track 538 last.
        assert_eq!(
            app.history[0].track.title, "Track 39",
            "the oldest entries are dropped, not the newest"
        );
        assert_eq!(
            app.history[HISTORY_CAP - 1].track.title,
            "Track 538",
            "the newest survive"
        );
    }

    #[test]
    fn interpolating_position_advances_while_playing() {
        let mut app = with_track();
        let before = app.interpolated_position();
        app.last_read = Some(Instant::now() - std::time::Duration::from_secs(2));
        assert!(app.interpolated_position() > before);
    }

    #[test]
    fn interpolation_never_runs_past_the_end() {
        let mut app = with_track();
        app.last_read = Some(Instant::now() - std::time::Duration::from_secs(99_999));
        let dur = app.track().unwrap().duration_secs() as f64;
        assert_eq!(app.interpolated_position(), dur);
    }

    #[test]
    fn interpolation_is_frozen_while_paused() {
        let mut app = with_track();
        if let Some(s) = app.state.as_mut() {
            s.playback = PlaybackState::Paused;
        }
        app.last_read = Some(Instant::now() - std::time::Duration::from_secs(5));
        assert!(app.interpolated_position() < 5.0);
    }

    #[test]
    fn keys_become_commands() {
        for (key, want) in [
            (' ', PlayerCommand::Toggle),
            ('n', PlayerCommand::Next),
            ('p', PlayerCommand::Prev),
            ('r', PlayerCommand::CycleRepeat),
            ('s', PlayerCommand::ToggleShuffle),
        ] {
            let (app, cmds) = press(with_track(), key);
            assert_eq!(cmds, vec![want], "key {key:?}");
            assert!(!app.should_quit);
        }
    }

    #[test]
    fn seek_keys_use_the_configured_step() {
        let (_, cmds) = press(with_track(), 'h');
        assert_eq!(cmds, vec![PlayerCommand::Seek(-5.0)]);
        let (_, cmds) = press(with_track(), 'l');
        assert_eq!(cmds, vec![PlayerCommand::Seek(5.0)]);
    }

    #[test]
    fn volume_keys_use_the_configured_step() {
        let (_, cmds) = press(with_track(), '+');
        assert_eq!(cmds, vec![PlayerCommand::VolumeStep(10)]);
        let (_, cmds) = press(with_track(), '-');
        assert_eq!(cmds, vec![PlayerCommand::VolumeStep(-10)]);
    }

    #[test]
    fn q_quits_and_nothing_else_does() {
        let (app, cmds) = press(with_track(), 'q');
        assert!(app.should_quit);
        assert!(cmds.is_empty());
    }

    /// COMPAT rule 2: the idle card's enter is the only launch path.
    #[test]
    fn enter_on_the_idle_card_launches_and_nothing_else_does() {
        let (app, cmds) = press(App::new(), '\n');
        assert_eq!(cmds, vec![PlayerCommand::Launch]);
        assert!(!app.should_quit);
        for other in ['x', 'z', '0', '\t', '?'] {
            let (_, cmds) = press(App::new(), other);
            assert!(
                cmds.is_empty(),
                "{other:?} must do nothing on the idle card"
            );
        }
    }

    /// COMPAT rule 3: nothing may write to Spotify without a user key.
    #[test]
    fn a_poll_or_a_tick_never_produces_a_command() {
        let (app, cmds) = step(with_track(), Event::Tick);
        assert!(cmds.is_empty());
        let (_, cmds) = step(app, Event::PlayerState(Box::new(playing())));
        assert!(cmds.is_empty(), "a read must never write back");
        let (_, cmds) = step(App::new(), Event::NotRunning);
        assert!(cmds.is_empty());
    }

    #[test]
    fn a_command_in_flight_blocks_another() {
        let (app, _) = press(with_track(), 'n');
        assert!(app.busy.is_some());
        let (_, cmds) = press(app, 'p');
        assert!(cmds.is_empty(), "a second command must not be queued");
    }

    #[test]
    fn finishing_a_command_clears_busy() {
        let (app, _) = press(with_track(), 'n');
        let app = update(
            app,
            Event::CommandDone(CommandOutcome::ok(PlayerCommand::Next)),
        )
        .app;
        assert!(app.busy.is_none());
    }

    #[test]
    fn mute_saves_and_restores_the_previous_volume() {
        let (mut app, cmds) = press(with_track(), 'm');
        assert_eq!(cmds, vec![PlayerCommand::SetVolume(0)]);
        assert!(app.muted);
        assert_eq!(app.pre_mute_volume, 100);

        // While muted, a read must not stomp the meter's value.
        let mut s = playing();
        s.volume = 42;
        app = update(app, Event::PlayerState(Box::new(s))).app;
        assert_eq!(app.meter_volume(), 0, "a read must not override a mute");

        // The first command has to finish before another can be queued; that is
        // the point of the busy flag, and the test has to honour it.
        let app = update(
            app,
            Event::CommandDone(CommandOutcome::read_back(
                PlayerCommand::SetVolume(0),
                landed("volume", 0),
            )),
        )
        .app;
        let (app, cmds) = press(app, 'm');
        assert_eq!(cmds, vec![PlayerCommand::SetVolume(100)]);
        assert!(!app.muted);
        assert_eq!(app.meter_volume(), 100, "unmute restores what the user had");
    }

    /// Tab goes forward and Shift-Tab back, which the event loop maps to 'Z'
    /// because nothing else is bound to it. The walk covers all eight tabs and
    /// comes back to where it started, so a tab added to the middle cannot be
    /// skipped by a `next` that stops early.
    #[test]
    fn tabs_cycle_and_numbers_jump() {
        let mut app = with_track();
        let start = app.tab;
        for i in 1..=Tab::ALL.len() {
            app = press(app, '\t').0;
            assert_eq!(
                app.tab,
                Tab::ALL[(start as usize + i) % Tab::ALL.len()],
                "after {i} tabs"
            );
        }
        assert_eq!(app.tab, start, "and the cycle comes back round");

        // Shift-Tab is the way back.
        let app = press(app, 'Z').0;
        let back = Tab::ALL
            .iter()
            .position(|t| *t == start)
            .map(|i| Tab::ALL[(i + Tab::ALL.len() - 1) % Tab::ALL.len()]);
        assert_eq!(Some(app.tab), back, "shift-tab goes back");

        // And a digit jumps, which is the same fact stated once rather than a
        // second table that can disagree with the first.
        for tab in Tab::ALL {
            let Some(digit) = tab.digit() else { continue };
            let (app, _) = press(with_track(), digit);
            assert_eq!(app.tab, tab, "{digit} should select {}", tab.label());
        }
        // History and Info have no digit: they are the `Tab` pair, and giving
        // them one would make the strip's numbers disagree with the order.
        assert_eq!(Tab::History.digit(), None);
        assert_eq!(Tab::Info.digit(), None);
        assert_eq!(
            press(with_track(), '7').0.tab,
            Tab::History,
            "7 is not a tab"
        );
    }

    #[test]
    fn history_cursor_clamps_at_both_ends() {
        let mut app = with_track();
        for n in 0..3 {
            let mut s = playing();
            s.track.uri = Some(format!("spotify:track:t{n}"));
            app = update(app, Event::PlayerState(Box::new(s))).app;
        }
        assert_eq!(app.history.len(), 3);
        for _ in 0..99 {
            let (next, _) = press(app, 'j');
            app = next;
        }
        // The newest of three rows is index 2. Clamping at `len` instead would
        // leave the cursor on a row that does not exist, with nothing selected.
        assert_eq!(app.history_cursor, 2, "must not point past the last row");
        for _ in 0..99 {
            let (next, _) = press(app, 'k');
            app = next;
        }
        assert_eq!(app.history_cursor, 0);
    }

    /// The cursor must always be one of the rows on screen, however long the
    /// session gets and whichever way it is scrolled.
    #[test]
    fn the_cursor_is_always_inside_the_rows_on_screen() {
        let mut app = with_track();
        for n in 0..(HISTORY_CAP + 10) {
            let mut s = playing();
            s.track.uri = Some(format!("spotify:track:t{n}"));
            app = update(app, Event::PlayerState(Box::new(s))).app;
        }
        app = update(app, Event::Viewport(9)).app;
        let check = |app: &App, how: &str| {
            let visible = app.visible_history().count();
            assert!(visible > 0, "{how}: nothing on screen");
            let first = app.history_cursor - app.history_scroll;
            assert!(
                first < visible,
                "{how}: the cursor is {first} rows into a page of {visible}"
            );
        };
        for _ in 0..(HISTORY_CAP + 20) {
            let (next, _) = press(app, 'j');
            app = next;
            check(&app, "after j");
        }
        for _ in 0..(HISTORY_CAP + 20) {
            let (next, _) = press(app, 'k');
            app = next;
            check(&app, "after k");
        }
        assert!(app.selected_history().is_some());
    }

    /// `n` skips, so the history holds `[Census, Old 0 .. Old n-2]` and the track
    /// now playing is `Old n-1`. The current track is *not* in the history -- it
    /// only gets in when the next one arrives -- which is the off-by-one every
    /// test below has to be written around.
    fn app_with_history(n: usize) -> App {
        let mut app = with_track();
        for i in 0..n {
            let mut s = playing();
            s.track.uri = Some(format!("spotify:track:t{i}"));
            s.track.title = format!("Old {i}");
            s.track.artist = "Someone".into();
            app = update(app, Event::PlayerState(Box::new(s))).app;
        }
        app
    }

    /// The whole point of the History tab (SPEC §4): `enter` plays that row again.
    #[test]
    fn enter_plays_the_selected_history_row() {
        let app = app_with_history(3);
        // Newest first, so row 0 is Old 1 -- Old 2 is what is playing now.
        assert_eq!(app.selected_history().unwrap().track.title, "Old 1");
        let (_, cmds) = press(app, '\n');
        assert_eq!(
            cmds,
            vec![PlayerCommand::PlayUri("spotify:track:t1".into())]
        );
    }

    /// Arrowing down must move the selection, and `enter` must follow it.
    #[test]
    fn enter_plays_wherever_the_cursor_is() {
        let app = app_with_history(3);
        let (app, _) = press(app, 'j');
        assert_eq!(app.selected_history().unwrap().track.title, "Old 0");
        let (_, cmds) = press(app, '\n');
        assert_eq!(
            cmds,
            vec![PlayerCommand::PlayUri("spotify:track:t0".into())]
        );
    }

    /// An advert has no URI, so there is nothing to ask Spotify to play.
    #[test]
    fn enter_does_nothing_when_the_row_is_unplayable() {
        let mut app = with_track();
        let mut ad = playing();
        ad.track.uri = None;
        app.history.push(HistoryEntry {
            track: ad.track,
            at: Instant::now(),
        });
        let (_, cmds) = press(app, '\n');
        assert!(cmds.is_empty(), "an advert must not be queued for playback");
    }

    /// Only the History tab has a selection, so enter elsewhere is a no-op rather
    /// than something that guesses.
    #[test]
    fn enter_is_only_meaningful_on_the_history_tab() {
        let mut app = app_with_history(2);
        app.tab = Tab::Info;
        let (_, cmds) = press(app, '\n');
        assert!(cmds.is_empty());
    }

    /// A new row lands at the front of the view, so the selection has to step
    /// down with it or it silently jumps to a different song.
    #[test]
    fn a_track_change_keeps_the_same_row_selected() {
        let app = app_with_history(3);
        let (app, _) = press(app, 'j');
        let chosen = app.selected_history().unwrap().track.title.clone();
        let mut s = playing();
        s.track.uri = Some("spotify:track:brand-new".into());
        s.track.title = "Brand New".into();
        let app = update(app, Event::PlayerState(Box::new(s))).app;
        assert_eq!(
            app.selected_history().unwrap().track.title,
            chosen,
            "a skip must not move the selection"
        );
    }

    /// The other half of that: a session nobody scrolled must leave the cursor
    /// on the newest row, not slowly walk down to the oldest one.
    #[test]
    fn an_untouched_cursor_stays_on_the_newest_row() {
        let app = app_with_history(6);
        assert_eq!(app.history.len(), 6);
        assert!(!app.cursor_moved);
        assert_eq!(
            app.selected_history().unwrap().track.title,
            "Old 4",
            "the newest finished track"
        );
        let (_, cmds) = press(app, '\n');
        assert_eq!(
            cmds,
            vec![PlayerCommand::PlayUri("spotify:track:t4".into())]
        );
    }

    #[test]
    fn times_heard_counts_the_current_track_and_the_past_ones() {
        let app = with_track();
        assert_eq!(app.times_heard(), Some(1), "heard once so far");

        // Listen to it again later, non-consecutively, so the dedupe cannot hide
        // the second play.
        let mut other = playing();
        other.track.uri = Some("spotify:track:something-else".into());
        let app = update(app, Event::PlayerState(Box::new(other))).app;
        let back = playing();
        let app = update(app, Event::PlayerState(Box::new(back))).app;
        assert_eq!(app.times_heard(), Some(2));
    }

    /// An advert has no URI, so "times heard" has nothing to count.
    #[test]
    fn times_heard_is_unknown_for_an_advert() {
        let ad = parse(&fixture("playing_ad.txt")).unwrap();
        let app = update(App::new(), Event::PlayerState(Box::new(ad))).app;
        assert_eq!(app.times_heard(), None);
    }

    /// Re-reading the same track must not inflate the count.
    #[test]
    fn a_poll_does_not_inflate_times_heard() {
        let mut app = with_track();
        for _ in 0..10 {
            app = update(app, Event::PlayerState(Box::new(playing()))).app;
        }
        assert_eq!(app.times_heard(), Some(1));
    }

    #[test]
    fn the_help_overlay_swallows_keys_then_closes() {
        let (app, _) = press(with_track(), '?');
        assert!(app.show_help);
        let (app, cmds) = press(app, 'n');
        assert!(cmds.is_empty(), "a key must not fire behind the overlay");
        assert!(!app.show_help);
    }

    /// SPEC §4: `esc` closes the overlay, and is inert everywhere else.
    #[test]
    fn esc_closes_the_overlay_and_is_inert_elsewhere() {
        let (app, _) = press(with_track(), '?');
        assert!(app.show_help);
        let (app, cmds) = press(app, '\x1b');
        assert!(!app.show_help, "esc closes it");
        assert!(cmds.is_empty(), "and fires nothing behind it");

        let (app, cmds) = press(with_track(), '\x1b');
        assert!(cmds.is_empty());
        assert!(!app.should_quit, "esc must not quit");
        assert_eq!(app.tab, Tab::History, "esc must not change anything");
    }

    #[test]
    fn q_still_quits_from_the_help_overlay() {
        let (app, _) = press(with_track(), '?');
        let (app, _) = press(app, 'q');
        assert!(app.should_quit);
    }

    #[test]
    fn a_failed_command_becomes_a_one_line_toast() {
        let (app, _) = press(with_track(), 'n');
        let app = update(
            app,
            Event::CommandDone(CommandOutcome::failed(
                PlayerCommand::Next,
                PlayerError::PermissionDenied,
            )),
        )
        .app;
        let t = app.toast.as_ref().expect("a toast");
        assert!(!t.text.contains('\n'), "{:?}", t.text);
    }

    #[test]
    fn a_toast_expires_on_a_tick() {
        let (app, _) = press(with_track(), 'n');
        let mut app = update(
            app,
            Event::CommandDone(CommandOutcome::failed(
                PlayerCommand::Next,
                PlayerError::NotRunning,
            )),
        )
        .app;
        assert!(app.toast.is_some());
        app.toast.as_mut().unwrap().at = Instant::now() - std::time::Duration::from_secs(60);
        let app = update(app, Event::Tick).app;
        assert!(app.toast.is_none());
    }

    #[test]
    fn not_running_returns_the_app_to_the_idle_card() {
        let (app, _) = press(with_track(), 'n');
        let app = update(app, Event::NotRunning).app;
        assert!(app.is_idle());
        assert!(app.last_read.is_none());
    }

    #[test]
    fn a_poll_is_only_due_when_nothing_is_in_flight() {
        let (app, _) = step(with_track(), Event::Tick);
        assert!(app.poll_due);
        let (app, _) = press(app, 'n');
        let app = update(app, Event::Tick).app;
        assert!(!app.poll_due, "a poll must not stack behind a command");
    }

    /// The shown mode advances on the keypress, not on the write coming back, so
    /// a repeated key needs the previous command to have finished.
    #[test]
    fn repeat_cycles_off_then_all_then_one() {
        let mut s = playing();
        s.repeating_enabled = false;
        let app = update(App::new(), Event::PlayerState(Box::new(s))).app;
        assert_eq!(app.repeat, RepeatMode::Off);

        let (app, cmds) = press(app, 'r');
        assert_eq!(cmds, vec![PlayerCommand::CycleRepeat]);
        assert_eq!(app.repeat, RepeatMode::Context, "all");

        let app = update(
            app,
            Event::CommandDone(CommandOutcome::ok(PlayerCommand::CycleRepeat)),
        )
        .app;
        let (app, _) = press(app, 'r');
        assert_eq!(app.repeat, RepeatMode::Track, "one");

        let app = update(
            app,
            Event::CommandDone(CommandOutcome::ok(PlayerCommand::CycleRepeat)),
        )
        .app;
        let (app, _) = press(app, 'r');
        assert_eq!(app.repeat, RepeatMode::Off, "and back round to off");
    }

    #[test]
    fn copy_shows_a_toast_with_the_share_url() {
        let (app, _) = press(with_track(), 'c');
        let t = app.toast.expect("a copied toast");
        assert!(
            t.text.starts_with("copied https://open.spotify.com/track/"),
            "{t:?}"
        );
    }

    #[test]
    fn copy_is_silent_when_nothing_playable_is_loaded() {
        // An advert has no URI, so there is no link to copy.
        let ad = parse(&fixture("playing_ad.txt")).unwrap();
        let app = update(App::new(), Event::PlayerState(Box::new(ad))).app;
        let (app, cmds) = press(app, 'c');
        assert!(cmds.is_empty());
        assert!(app.toast.is_none());
    }

    /// A click has to do what the pixel under it looks like it does.
    #[test]
    fn a_click_does_what_the_thing_under_it_says() {
        let dur = with_track().track().unwrap().duration_secs() as f64;
        assert!(dur > 0.0);

        // The three transport controls, each from a clean app so nothing is in
        // flight.
        for (target, want) in [
            (Hit::Control(Control::Prev), PlayerCommand::Prev),
            (Hit::Control(Control::Toggle), PlayerCommand::Toggle),
            (Hit::Control(Control::Next), PlayerCommand::Next),
        ] {
            let (_, cmds) = step(with_track(), click(target));
            assert_eq!(cmds, vec![want], "{target:?}");
        }

        // Halfway along the progress bar is halfway through the track.
        let (_, cmds) = step(with_track(), click(Hit::Seek(0.5)));
        assert_eq!(cmds, vec![PlayerCommand::Seek((dur * 0.5).round())]);

        // And a click past the end clamps rather than seeking outside the track.
        let (_, cmds) = step(with_track(), click(Hit::Seek(1.4)));
        assert_eq!(cmds, vec![PlayerCommand::Seek(dur)]);
        let (_, cmds) = step(with_track(), click(Hit::Seek(-0.2)));
        assert_eq!(cmds, vec![PlayerCommand::Seek(0.0)]);
    }

    fn click(target: Hit) -> Event {
        Event::Mouse(Mouse {
            action: MouseAction::Press,
            target,
        })
    }

    fn scroll(down: bool) -> Event {
        Event::Mouse(Mouse {
            action: if down {
                MouseAction::ScrollDown
            } else {
                MouseAction::ScrollUp
            },
            target: Hit::HistoryPane,
        })
    }

    #[test]
    fn a_tab_click_switches_tab_and_a_bad_index_does_nothing() {
        // The click index and the digit are the same fact, so they are compared
        // against the same table: a strip that draws in one order and hits in
        // another is the bug this catches.
        let (app, cmds) = step(with_track(), click(Hit::Tab(2)));
        assert_eq!(app.tab, Tab::Queue);
        assert!(cmds.is_empty(), "switching tab is not a Spotify write");
        let (app, _) = step(app, click(Hit::Tab(99)));
        assert_eq!(
            app.tab,
            Tab::Queue,
            "an index that does not exist is ignored"
        );
    }

    /// The overlay is modal: a click behind it must not reach the dashboard.
    #[test]
    fn a_click_behind_the_overlay_only_closes_it() {
        let (app, _) = press(with_track(), '?');
        let want = fingerprint(&app);
        let (app, cmds) = step(app, click(Hit::Control(Control::Next)));
        assert!(cmds.is_empty(), "a click must not fire behind the overlay");
        assert!(!app.show_help, "and it closes the overlay");
        assert_eq!(app.tab, want.tab);
    }

    /// A release is not a command: the drag already happened on the press.
    #[test]
    fn releasing_the_button_is_not_a_second_command() {
        let (_, cmds) = step(
            with_track(),
            Event::Mouse(Mouse {
                action: MouseAction::Release,
                target: Hit::Control(Control::Next),
            }),
        );
        assert!(cmds.is_empty());
    }

    /// Clicking a row selects it, counted from what is on screen — which is the
    /// whole point of a scrollable list.
    #[test]
    fn clicking_a_row_selects_it_where_it_is_shown() {
        let mut app = app_with_history(20);
        app = step(app, Event::Viewport(5)).0;
        // A few rows down, so the view has scrolled.
        for _ in 0..8 {
            app = press(app, 'j').0;
        }
        let scroll = app.history_scroll;
        assert!(
            scroll > 0,
            "the view should have scrolled to follow the cursor"
        );
        let (app, cmds) = step(app.clone(), click(Hit::HistoryRow(0)));
        assert!(cmds.is_empty(), "a click selects, it does not play");
        assert_eq!(
            app.history_cursor, scroll,
            "row 0 of the view is the top row that is showing"
        );
        let (app, _) = step(app, click(Hit::HistoryRow(2)));
        assert_eq!(app.history_cursor, scroll + 2);
    }

    /// A wheel over the list moves the selection, and the view follows it.
    #[test]
    fn the_wheel_scrolls_the_selection_and_the_view() {
        let mut app = app_with_history(40);
        app = step(app, Event::Viewport(6)).0;
        let (app, cmds) = step(app, scroll(true));
        assert!(cmds.is_empty(), "scrolling is not a Spotify write");
        assert_eq!(app.history_cursor, 3, "three lines a notch");

        let (app, _) = step(app, scroll(true));
        assert_eq!(app.history_cursor, 6);
        assert_eq!(app.history_scroll, 1, "and the view followed it");

        let (app, _) = step(app, scroll(false));
        assert_eq!(app.history_cursor, 3);
        // The view does not re-centre: row 3 is still on screen from row 1, so
        // it stays put. Scrolling only when the cursor would leave is the
        // behaviour you can predict.
        assert_eq!(app.history_scroll, 1);

        // Far enough up and the view has to follow.
        let app = app_with_history(40);
        let (app, _) = step(app, Event::Viewport(6));
        let mut app = app;
        for _ in 0..5 {
            app = press(app, 'k').0;
        }
        assert_eq!(app.history_scroll, 0, "scrolled back to the top");
    }

    /// The history can be 500 long and the pane 20 rows: the cursor must never
    /// walk off the screen, and the view must never scroll past the end.
    #[test]
    fn the_view_follows_the_cursor_over_a_long_history() {
        let mut app = with_track();
        for i in 0..HISTORY_CAP {
            let mut s = playing();
            s.track.uri = Some(format!("spotify:track:t{i}"));
            app = update(app, Event::PlayerState(Box::new(s))).app;
        }
        let page = 8;
        app = step(app, Event::Viewport(page)).0;
        for _ in 0..(HISTORY_CAP + 50) {
            app = press(app, 'j').0;
            assert!(
                app.history_cursor < app.history_scroll + page,
                "the cursor at {} is off the view starting at {}",
                app.history_cursor,
                app.history_scroll
            );
        }
        assert_eq!(app.history_cursor, HISTORY_CAP - 1);
        // The last page is full of history, not half history and half nothing.
        let last = HISTORY_CAP - page;
        assert_eq!(app.history_scroll, last);
        assert_eq!(app.visible_history().count(), page);
    }

    /// A resize must not leave the view scrolled past the end of the list.
    #[test]
    fn growing_the_pane_keeps_the_view_inside_the_list() {
        let mut app = with_track();
        for i in 0..10 {
            let mut s = playing();
            s.track.uri = Some(format!("spotify:track:t{i}"));
            app = update(app, Event::PlayerState(Box::new(s))).app;
        }
        app = step(app, Event::Viewport(2)).0;
        for _ in 0..9 {
            app = press(app, 'j').0;
        }
        assert!(app.history_scroll > 0);
        // Now the pane is huge: the whole list fits, so there is nothing to
        // scroll.
        let app = step(app, Event::Viewport(200)).0;
        assert_eq!(app.history_scroll, 0, "nothing to scroll when it all fits");
        assert_eq!(app.visible_history().count(), app.history.len());
    }

    /// COMPAT rule 3: a mouse event that arrives while the setting is off must
    /// not do anything. The loop also does not ask for events, so this is belt
    /// and braces -- but a stray event from a terminal that ignores the setting
    /// must not become a Spotify write.
    #[test]
    fn mouse_events_are_ignored_when_the_setting_is_off() {
        let mut app = with_track();
        app.settings.mouse = false;
        let want = fingerprint(&app);
        for m in [
            Mouse {
                action: MouseAction::Press,
                target: Hit::Control(Control::Next),
            },
            Mouse {
                action: MouseAction::Drag,
                target: Hit::Seek(0.5),
            },
            Mouse {
                action: MouseAction::ScrollDown,
                target: Hit::HistoryPane,
            },
            Mouse {
                action: MouseAction::Press,
                target: Hit::Tab(2),
            },
        ] {
            let (next, cmds) = step(app, Event::Mouse(m));
            assert!(cmds.is_empty(), "{m:?} wrote to Spotify with mouse off");
            assert_eq!(fingerprint(&next), want, "{m:?} changed the app");
            app = next;
        }
    }

    /// Everything about the app that a keypress could change.
    ///
    /// A struct rather than a tuple: std only derives `Debug` and `PartialEq`
    /// for tuples up to twelve elements, and comparing three or four fields is
    /// how a test ends up passing while a key quietly starts doing something.
    /// **A new state field belongs in here.**
    #[derive(Debug, PartialEq)]
    struct Fingerprint {
        loaded: bool,
        tab: Tab,
        history: usize,
        cursor: usize,
        cursor_moved: bool,
        help: bool,
        user_volume: Option<u8>,
        read_volume: u8,
        volume_hidden: bool,
        muted: bool,
        pre_mute_volume: u8,
        repeat: RepeatMode,
        busy: bool,
        toast: bool,
        quit: bool,
        poll_due: bool,
        read_at: bool,
    }

    fn fingerprint(app: &App) -> Fingerprint {
        Fingerprint {
            loaded: app.state.is_some(),
            tab: app.tab,
            history: app.history.len(),
            cursor: app.history_cursor,
            cursor_moved: app.cursor_moved,
            help: app.show_help,
            user_volume: app.user_volume,
            read_volume: app.read_volume,
            volume_hidden: app.volume_hidden,
            muted: app.muted,
            pre_mute_volume: app.pre_mute_volume,
            repeat: app.repeat,
            busy: app.busy.is_some(),
            toast: app.toast.is_some(),
            quit: app.should_quit,
            poll_due: app.poll_due,
            read_at: app.last_read.is_some(),
        }
    }

    /// Every key the help overlay excuses as "not built yet" must genuinely do
    /// nothing at all. An excuse that has quietly become false would let a key
    /// fall out of the help without anything failing.
    #[test]
    fn every_excused_key_really_is_inert() {
        let excused: Vec<String> = crate::tui::render::NOT_YET
            .iter()
            .map(|(k, _)| k.to_string())
            .collect();
        assert!(!excused.is_empty(), "the excuse list should not be empty");
        for key in excused {
            let want = fingerprint(&with_track());
            let (app, cmds) = press(with_track(), key.chars().next().unwrap());
            assert!(
                cmds.is_empty(),
                "`{key}` is excused as unbound but queued {cmds:?}"
            );
            assert_eq!(
                fingerprint(&app),
                want,
                "`{key}` is excused as unbound but changed the app"
            );
        }
    }

    /// A stand-in for a fetched cover. The pixels never matter to the app; only
    /// the identity does.
    fn loaded_art(path: &str) -> crate::player::actions::LoadedArt {
        crate::player::actions::LoadedArt {
            path: path.into(),
            image: image::DynamicImage::ImageRgb8(image::RgbImage::new(2, 2)),
        }
    }

    /// The notification is off by default (SPEC §8) and must never announce the
    /// first read of a session: starting trak is not a song change.
    #[test]
    fn the_song_change_notification_is_off_until_asked_for() {
        let app = with_track();
        assert!(
            !app.settings.song_change_notification,
            "off by default, per SPEC section 8"
        );
        let (mut app, cmds) = step(app, Event::SoundForTrackChanged);
        assert!(cmds.is_empty(), "nothing should be announced");
        app.settings.song_change_notification = true;
        let (_, cmds) = step(app, Event::SoundForTrackChanged);
        assert_eq!(
            cmds,
            vec![PlayerCommand::Notify(
                "Census Designated".into(),
                "Jane Remover — Census Designated".into(),
            )],
            "title is the track, body is artist and album"
        );
    }

    /// Every shape of a track has to produce a notification that is not
    /// nonsense, and a promo with no artist and no album is the awkward one.
    #[test]
    fn the_notification_body_copes_with_missing_fields() {
        for (artist, album, want) in [
            (
                "Jane Remover",
                "Census Designated",
                "Jane Remover — Census Designated",
            ),
            ("Jane Remover", "", "Jane Remover"),
            ("", "Some Album", ""),
            ("", "", ""),
        ] {
            let mut app = with_track();
            app.settings.song_change_notification = true;
            let mut s = playing();
            s.track.title = "Some Track".into();
            s.track.artist = artist.into();
            s.track.album = album.into();
            app = update(app, Event::PlayerState(Box::new(s))).app;
            let (_, cmds) = step(app, Event::SoundForTrackChanged);
            match cmds.as_slice() {
                [PlayerCommand::Notify(title, body)] => {
                    assert_eq!(title, "Some Track");
                    assert_eq!(body, want, "{artist:?}/{album:?}");
                }
                other => panic!("expected exactly one notification, got {other:?}"),
            }
        }
    }

    /// On the system control the volume keys are somebody else's volume, so trak
    /// says so rather than silently doing nothing (TODO 4.4, R2).
    #[test]
    fn the_system_volume_control_explains_itself() {
        let mut app = with_track();
        app.settings.volume_control = VolumeControl::System;
        for key in ['+', '=', '-', '_'] {
            let (app, cmds) = press(app.clone(), key);
            assert!(
                cmds.is_empty(),
                "{key:?} must not silently change the system volume"
            );
            let t = app.toast.as_ref().expect("an explanation");
            assert!(t.text.contains("system"), "{t:?}");
        }
        // And on Spotify's own control the keys work exactly as before.
        let mut app = with_track();
        app.settings.volume_control = VolumeControl::Spotify;
        let (_, cmds) = press(app, '+');
        assert_eq!(cmds, vec![PlayerCommand::VolumeStep(10)]);
    }

    #[test]
    fn the_volume_control_parses_the_config_strings() {
        assert_eq!(
            VolumeControl::parse("spotify"),
            Some(VolumeControl::Spotify)
        );
        assert_eq!(
            VolumeControl::parse(" system "),
            Some(VolumeControl::System)
        );
        assert_eq!(VolumeControl::parse("everything"), None);
        assert_eq!(VolumeControl::System.label(), "system");
    }

    /// COMPAT rule 3 in one test: while Sonar owns the volume, trak must not
    /// write one. Not the mute alone -- a *relative* step is computed from the
    /// live volume, which mid-fade is Sonar's own value, so pressing `-` would
    /// write a mid-fade number back and trak would be undoing Sonar's fade.
    #[test]
    fn no_volume_write_while_sonar_is_ducking() {
        for key in ['m', '+', '=', '-', '_'] {
            let mut app = with_track();
            app.sonar = crate::sonar::SonarState {
                phase: crate::sonar::SonarPhase::Ducking,
                since: None,
            };
            let want = fingerprint(&app);
            let (app, cmds) = press(app, key);
            assert!(cmds.is_empty(), "{key:?} wrote to Spotify during a duck");
            assert!(!app.muted, "{key:?} muted anyway");
            assert_eq!(app.user_volume, want.user_volume, "{key:?} moved the meter");
            assert_eq!(app.read_volume, want.read_volume, "{key:?}");
            assert_eq!(app.tab, want.tab, "{key:?}");
            // The one thing it may do is say why it did nothing: a key that
            // silently stops working looks like a broken keyboard.
            let t = app.toast.as_ref().expect("an explanation");
            assert!(!t.text.contains('\n'), "{t:?}");
            assert!(t.text.contains("Sonar"), "{t:?}");
        }
    }

    /// The moment a duck begins is worth saying out loud, because it is the
    /// reason the volume keys just stopped working -- and it says it once, not
    /// every time the state file is re-read.
    #[test]
    fn a_new_duck_says_so_once() {
        let app = with_track();
        assert!(!app.sonar.is_ducking());
        let ducking = crate::sonar::SonarState {
            phase: crate::sonar::SonarPhase::Ducking,
            since: None,
        };
        let app = update(app, Event::Sonar(ducking)).app;
        assert!(app.sonar.is_ducking());
        let t = app.toast.as_ref().expect("a toast");
        assert!(!t.text.contains('\n'), "{t:?}");

        let toasts_before = fingerprint(&app).toast;
        let app = update(app, Event::Sonar(ducking)).app;
        assert_eq!(
            fingerprint(&app).toast,
            toasts_before,
            "and it does not nag"
        );
    }

    /// A duck that has finished must hand the volume keys back.
    #[test]
    fn the_volume_comes_back_when_the_duck_ends() {
        let mut app = with_track();
        app.sonar = crate::sonar::SonarState {
            phase: crate::sonar::SonarPhase::Ducking,
            since: None,
        };
        let (app, cmds) = press(app, '-');
        assert!(cmds.is_empty(), "refused while ducking");
        let app = update(
            app,
            Event::Sonar(crate::sonar::SonarState {
                phase: crate::sonar::SonarPhase::Idle,
                since: None,
            }),
        )
        .app;
        let (_, cmds) = press(app, '-');
        assert_eq!(
            cmds,
            vec![PlayerCommand::VolumeStep(-10)],
            "and it works again"
        );
    }

    /// Nothing here is fatal. A missing Sonar, a missing headless-spotify, a
    /// state file from a future version: all of it is a state trak shows.
    #[test]
    fn a_missing_sibling_is_not_an_error() {
        let app = with_track();
        assert!(!app.sonar.is_known());
        assert!(app.sonar.badge().is_none());
        assert!(!app.headless.installed);
        assert!(app.headless.badge().is_none());
        assert!(app.headless.hint().is_none());
    }

    /// A sibling that *is* there gets a badge, and the badge is not a guess.
    #[test]
    fn a_present_sibling_gets_a_badge() {
        let mut app = with_track();
        app.sonar = crate::sonar::SonarState {
            phase: crate::sonar::SonarPhase::Idle,
            since: None,
        };
        assert_eq!(app.sonar.badge(), Some("sonar"));
        app.headless = crate::headless::Headless {
            installed: true,
            hidden: Some(true),
            running: Some(true),
            schema: Some(1),
        };
        assert_eq!(app.headless.badge(), Some("headless"));
        assert!(app.headless.hint().is_some());
    }

    /// The badge says a sibling is there; the notice says what it is doing. Only
    /// one of those belongs on one line of a header.
    #[test]
    fn the_badge_is_short_and_the_notice_is_one_line() {
        let s = crate::sonar::SonarState {
            phase: crate::sonar::SonarPhase::Resuming,
            since: None,
        };
        let badge = s.badge().expect("a badge");
        assert!(
            badge.chars().count() <= 8,
            "a badge has to fit a header: {badge:?}"
        );
        assert!(!s.notice().contains('\n'), "{}", s.notice());
    }

    /// A read-back that says the write took.
    fn landed(what: &'static str, wanted: i64) -> WriteOutcome {
        WriteOutcome {
            what,
            wanted,
            read: wanted,
            landed: true,
        }
    }

    /// A read-back that says Spotify took the call and did nothing with it.
    fn ignored(what: &'static str, wanted: i64, read: i64) -> WriteOutcome {
        WriteOutcome {
            what,
            wanted,
            read,
            landed: false,
        }
    }

    /// The regression COMPAT rule 3 exists for: Sonar fades the volume while
    /// ducking, trak polls, and the meter must not follow the fade down. If it
    /// did, the user would think trak had turned the music down.
    #[test]
    fn a_poll_never_moves_a_volume_the_user_chose() {
        let (mut app, cmds) = press(with_track(), '-');
        assert_eq!(cmds, vec![PlayerCommand::VolumeStep(-10)]);
        app = update(
            app,
            Event::CommandDone(CommandOutcome::read_back(
                PlayerCommand::VolumeStep(-10),
                landed("volume", 90),
            )),
        )
        .app;
        assert_eq!(app.meter_volume(), 90, "the user asked for 90");

        // Sonar ducks: Spotify's own volume falls away, twice.
        for faded in [45, 12] {
            let mut s = playing();
            s.volume = faded;
            app = update(app, Event::PlayerState(Box::new(s))).app;
            assert_eq!(app.read_volume, faded, "the read is recorded");
            assert_eq!(
                app.meter_volume(),
                90,
                "but the meter shows what the user chose, not a mid-fade value"
            );
        }
    }

    /// The same rule the other way round: with no choice of the user's own, the
    /// meter still has to show something true.
    #[test]
    fn the_meter_falls_back_to_what_spotify_reports() {
        let mut app = with_track();
        assert_eq!(app.user_volume, None);
        assert_eq!(app.meter_volume(), 100);
        let mut s = playing();
        s.volume = 33;
        app = update(app, Event::PlayerState(Box::new(s))).app;
        assert_eq!(app.meter_volume(), 33);
    }

    /// COMPAT rule 5: an ignored volume write hides the meter and says why.
    #[test]
    fn an_ignored_volume_write_hides_the_meter_and_explains_itself() {
        let (app, _) = press(with_track(), '-');
        let app = update(
            app,
            Event::CommandDone(CommandOutcome::read_back(
                PlayerCommand::VolumeStep(-10),
                ignored("volume", 90, 100),
            )),
        )
        .app;
        assert!(app.volume_hidden, "the meter would be a lie");
        let t = app.toast.as_ref().expect("a notice");
        assert!(!t.text.contains('\n'), "one line only: {t:?}");
        assert!(t.text.contains("volume"), "{t:?}");
    }

    /// A write that lands brings the meter back, so one bad keypress is not a
    /// permanently broken display.
    #[test]
    fn a_landed_write_brings_the_meter_back() {
        let (mut app, _) = press(with_track(), '-');
        app = update(
            app,
            Event::CommandDone(CommandOutcome::read_back(
                PlayerCommand::VolumeStep(-10),
                ignored("volume", 90, 100),
            )),
        )
        .app;
        assert!(app.volume_hidden);

        let (app, _) = press(app, '-');
        let app = update(
            app,
            Event::CommandDone(CommandOutcome::read_back(
                PlayerCommand::VolumeStep(-10),
                landed("volume", 80),
            )),
        )
        .app;
        assert!(!app.volume_hidden);
        assert_eq!(app.meter_volume(), 80);
    }

    /// Spotify quantises, so a read-back one below what was asked for is a
    /// success (COMPAT rule 5). This test is the reason the meter never hid
    /// itself on this machine.
    #[test]
    fn a_quantised_read_back_does_not_hide_the_meter() {
        let (app, _) = press(with_track(), '-');
        let app = update(
            app,
            Event::CommandDone(CommandOutcome::read_back(
                PlayerCommand::VolumeStep(-10),
                WriteOutcome {
                    what: "volume",
                    wanted: 90,
                    read: 89,
                    landed: true,
                },
            )),
        )
        .app;
        assert!(!app.volume_hidden, "one low is still landed");
        assert_eq!(app.meter_volume(), 90, "the value set, not the read");
    }

    /// An ignored seek is worth a notice, but it is not a reason to believe the
    /// volume meter has stopped working.
    #[test]
    fn an_ignored_seek_does_not_hide_the_volume_meter() {
        let (app, _) = press(with_track(), 'l');
        let app = update(
            app,
            Event::CommandDone(CommandOutcome::read_back(
                PlayerCommand::Seek(5.0),
                ignored("seek", 5, 0),
            )),
        )
        .app;
        assert!(!app.volume_hidden);
        assert!(app.toast.is_some(), "but the user is told");
    }

    /// The value the player actually aimed for is the one shown, so the clamp
    /// lives in one place.
    #[test]
    fn the_meter_shows_the_clamped_target_the_player_aimed_for() {
        let mut app = with_track();
        app.user_volume = Some(95);
        let (app, _) = press(app, '+');
        let app = update(
            app,
            Event::CommandDone(CommandOutcome::read_back(
                PlayerCommand::VolumeStep(10),
                landed("volume", 100),
            )),
        )
        .app;
        assert_eq!(app.meter_volume(), 100, "not 105");
    }

    /// The cover must belong to the track that is playing, or a slow download
    /// shows the previous album under the new title.
    #[test]
    fn an_image_is_only_drawn_for_its_own_track() {
        let mut app = with_track();
        let url = app.track().unwrap().artwork_url.clone().unwrap();
        assert!(app.art.wants(app.track().unwrap()), "nothing fetched yet");
        assert!(app.art.drawable(app.track().unwrap()).is_none());

        let art = loaded_art("/tmp/art.img");
        assert!(app.art.begin(&url), "the fetch is started for this track");
        let app = step(
            app,
            Event::Art {
                url,
                result: Ok(art.clone()),
            },
        )
        .0;
        assert_eq!(app.art.drawable(app.track().unwrap()), Some(&art));

        // Now the track changes. The old file is still on disk, and drawing it
        // under the new title would be a lie.
        let mut other = playing();
        other.track.artwork_url = Some("https://i.scdn.co/image/other".into());
        let app = step(app, Event::PlayerState(Box::new(other))).0;
        assert!(
            app.art.drawable(app.track().unwrap()).is_none(),
            "the previous cover must not be shown under the new title"
        );
        assert!(
            app.art.wants(app.track().unwrap()),
            "and the new one is wanted"
        );
    }

    /// A fetch for a track the user skipped past is dropped, cover and all.
    #[test]
    fn an_image_for_a_track_we_have_left_is_ignored() {
        let app = with_track();
        let stale = "https://i.scdn.co/image/stale".to_string();
        let mut other = playing();
        other.track.artwork_url = Some(stale.clone());
        let mut app = step(app, Event::PlayerState(Box::new(other))).0;
        assert!(app.art.begin(&stale));

        let app = step(
            app,
            Event::Art {
                url: "https://i.scdn.co/image/something-else".into(),
                result: Ok(loaded_art("/tmp/other.img")),
            },
        )
        .0;
        assert!(
            app.art.loaded.is_none(),
            "a stale download must not be adopted"
        );
        assert!(
            app.art.wants(app.track().unwrap()),
            "the real one is still owed"
        );
    }

    /// A failed fetch is a placeholder and one line, never a crash.
    #[test]
    fn a_failed_fetch_keeps_the_dashboard_and_explains_itself() {
        let app = with_track();
        let url = app.track().unwrap().artwork_url.clone().unwrap();
        let app = step(
            app,
            Event::Art {
                url,
                result: Err(crate::art::ArtError::Unreachable),
            },
        )
        .0;
        assert!(!app.art.loading);
        assert!(app.art.loaded.is_none());
        let e = app.art.error.as_deref().expect("a reason");
        assert!(!e.contains('\n'), "one line: {e:?}");
        assert!(app.track().is_some(), "the rest of the dashboard is intact");
        // And it is not retried in a loop: `wants` stays true only because there
        // is no image, and the loop asks once per read.
        assert!(app.art.wants(app.track().unwrap()));
    }

    /// A track with no artwork asks for nothing. An advert is the common case.
    #[test]
    fn a_track_with_no_artwork_asks_for_nothing() {
        let ad = parse(&fixture("playing_ad.txt")).unwrap();
        let app = update(App::new(), Event::PlayerState(Box::new(ad))).app;
        assert!(!app.art.wants(app.track().unwrap()));
    }

    /// A fetch in flight is not started twice, or a skip would queue a download
    /// per poll.
    #[test]
    fn a_fetch_in_flight_is_not_started_again() {
        let mut app = with_track();
        let url = app.track().unwrap().artwork_url.clone().unwrap();
        // The loop asks `wants`, then `begin`, then submits the download.
        assert!(app.art.wants(app.track().unwrap()));
        assert!(app.art.begin(&url), "the first request goes through");
        assert!(app.art.loading);
        assert!(!app.art.wants(app.track().unwrap()), "already in flight");

        let app = step(
            app,
            Event::Art {
                url,
                result: Ok(loaded_art("/a")),
            },
        )
        .0;
        assert!(!app.art.loading);
        assert!(
            !app.art.wants(app.track().unwrap()),
            "already have it, so no second download"
        );

        // A new track is a new download, and the old cover is not adopted.
        let mut other = playing();
        other.track.artwork_url = Some("https://i.scdn.co/image/two".into());
        let mut app = step(app, Event::PlayerState(Box::new(other))).0;
        assert!(app.art.wants(app.track().unwrap()));
        assert!(app.art.begin("https://i.scdn.co/image/two"));
        assert!(
            !app.art.begin("https://i.scdn.co/image/two"),
            "one download per track, not one per poll"
        );
    }

    #[test]
    fn the_settings_defaults_match_spec() {
        let s = Settings::default();
        assert_eq!(s.seek_step, 5.0);
        assert_eq!(s.volume_step, 10);
        assert!(s.rounded(), "rounded borders are the SPEC default");
        assert!(s.show_clock && s.show_volume && s.show_key_hints && s.side_pane);
    }

    #[test]
    fn the_sample_track_is_complete() {
        let t = sample_track();
        assert!(t.uri.is_some() && t.artwork_url.is_some());
        assert!(!t.is_ad());
    }
}

#[cfg(test)]
mod lyrics_tests {
    use super::*;
    use crate::lyrics::index_at;

    fn playing() -> PlayerState {
        crate::player::parse::parse(&crate::testutil::fixture("playing_track.txt")).unwrap()
    }

    fn with_track() -> App {
        update(App::new(), Event::PlayerState(Box::new(playing()))).app
    }

    fn lyrics_for(texts: &[(&str, f64)]) -> crate::lyrics::Lyrics {
        crate::lyrics::Lyrics {
            lines: texts
                .iter()
                .map(|(t, s)| crate::lyrics::LyricLine {
                    time_secs: *s,
                    text: (*t).to_string(),
                })
                .collect(),
            synced: true,
            source: "test".into(),
            instrumental: false,
        }
    }

    /// The whole point of the tab: the line being sung at 0:30 is the one that
    /// starts at 0:29, not the next one and not the one before.
    #[test]
    fn the_active_line_follows_the_music() {
        let app = with_track();
        let l = lyrics_for(&[("one", 10.0), ("two", 20.0), ("three", 30.0)]);
        for (at, want) in [
            (5.0, None),
            (10.0, Some(0)),
            (19.9, Some(0)),
            (20.0, Some(1)),
            (35.0, Some(2)),
        ] {
            assert_eq!(index_at(&l, at), want, "at {at}s");
        }
        let _ = app;
    }

    /// A track's lyrics must not survive into the next track. Carrying a chorus
    /// over is worse than showing nothing.
    #[test]
    fn a_new_track_takes_its_lyrics_with_it() {
        let mut app = with_track();
        let uri = app.track().unwrap().uri.clone();
        app = update(
            app,
            Event::Lyrics {
                uri: uri.clone(),
                result: Ok(lyrics_for(&[("hello", 1.0)])),
            },
        )
        .app;
        assert_eq!(app.lyrics.status, LyricsStatus::Ready);
        assert!(app.lyrics.active(2.0).is_some());

        let mut other = playing();
        other.track.uri = Some("spotify:track:NEXT".into());
        app = update(app, Event::PlayerState(Box::new(other))).app;
        assert_eq!(
            app.lyrics.status,
            LyricsStatus::Idle,
            "cleared on a new track"
        );
        assert!(app.lyrics.lyrics.is_none());
        assert_eq!(app.lyrics.active(2.0), None);
    }

    /// Lyrics that arrive after the user has skipped past are dropped rather than
    /// shown under a title they do not belong to.
    #[test]
    fn lyrics_for_a_track_we_have_left_are_dropped() {
        let app = with_track();
        let app = update(
            app,
            Event::Lyrics {
                uri: Some("spotify:track:SOMETHING-ELSE".into()),
                result: Ok(lyrics_for(&[("stale", 1.0)])),
            },
        )
        .app;
        assert_eq!(app.lyrics.status, LyricsStatus::Idle, "nothing was here");
        assert!(app.lyrics.lyrics.is_none());
    }

    /// "Not in the database" is the common case, and it is not a failure.
    #[test]
    fn not_found_is_its_own_state() {
        let mut app = with_track();
        let uri = app.track().unwrap().uri.clone();
        app = update(
            app,
            Event::Lyrics {
                uri,
                result: Err(crate::lyrics::LyricsError::NotFound),
            },
        )
        .app;
        assert_eq!(app.lyrics.status, LyricsStatus::NotFound);
        // And the tab can still ask again on the next track.
        let mut other = playing();
        other.track.uri = Some("spotify:track:NEXT".into());
        let app = update(app, Event::PlayerState(Box::new(other))).app;
        assert_eq!(app.lyrics.status, LyricsStatus::Idle);
    }

    #[test]
    fn a_failure_says_why_in_one_line() {
        let mut app = with_track();
        let uri = app.track().unwrap().uri.clone();
        app = update(
            app,
            Event::Lyrics {
                uri,
                result: Err(crate::lyrics::LyricsError::Unreachable),
            },
        )
        .app;
        match &app.lyrics.status {
            LyricsStatus::Failed(why) => assert!(!why.contains('\n'), "{why:?}"),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    /// Switching lyrics off means never looking, not looking and hiding it.
    #[test]
    fn lyrics_can_be_switched_off() {
        let mut app = with_track();
        app.settings.lyrics = false;
        assert!(!app.settings.lyrics);
        assert!(
            Settings::default().lyrics,
            "on by default, per SPEC section 8"
        );
    }
}
