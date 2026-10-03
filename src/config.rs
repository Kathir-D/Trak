//! trak's settings file: `~/.config/trak/config.toml` (SPEC §8, TODO 5.1).
//!
//! Four rules keep this module small enough not to need a dependency on it:
//!
//! - **A missing file is every default.** That is the first-run path (TODO 5.4)
//!   and it is the common path: most people never open `trak config`, so the
//!   file is optional by design and its absence can never be a failure.
//! - **Unknown keys and unknown tables are ignored.** The file is written by one
//!   trak and read by another, and humans edit it by hand, so a key a future
//!   version adds must not make this version refuse the file.
//! - **A wrong value is a default, not a broken file.** `volume = "loud"` is one
//!   mistyped key; the other forty are still good, and throwing the file away
//!   over one of them would be the worst possible answer.
//! - **Only a file that cannot be parsed at all is corrupt**, and that one is
//!   renamed to `config.toml.bak` rather than deleted: a person wrote it and may
//!   want it back.
//!
//! TOML is read and written here rather than through `serde` and `toml`, which
//! the crate does not have on purpose. What is implemented is the subset this
//! schema needs -- tables, `key = value`, comments, strings, integers, floats,
//! booleans and lists of those -- with the same reasoning as the hand-written
//! JSON readers in `sonar.rs`, `headless.rs` and `lyrics.rs`: a dependency for
//! thirty keys of one flat table each is not worth it.
//!
//! Everything that decides anything is pure. [`parse`] takes bytes,
//! [`Config::to_toml`] takes a [`Config`], and the mapping onto the TUI's
//! [`Settings`] is arithmetic. Only [`Config::load_from`] and
//! [`Config::save_to`] touch the filesystem, and the path they use comes in
//! through [`Paths`] rather than from `std::env`, so the tests never read or
//! write the owner's own config.

use std::fs::{self, File};
use std::io::{Read, Write as _};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::tui::app::{DisplayMode, Settings, Tab, VisualizerStyle, VolumeControl};
use crate::tui::theme::{Accent, Border};

/// The file inside the config directory.
pub const FILE_NAME: &str = "config.toml";

/// What a file that could not be parsed is renamed to.
pub const BACKUP_SUFFIX: &str = ".bak";

/// The directory inside `XDG_CONFIG_HOME` or `HOME/.config` that trak keeps.
const DIR_NAME: &str = "trak";

/// Refuse a file larger than this. The schema is thirty keys of a few bytes
/// each, so anything this size is not a config; it also stops a file that is not
/// a config -- a symlink to a log, a swap file -- from being read at all.
pub const MAX_BYTES: u64 = 64 * 1024;

/// How deep a list may nest before the reader refuses the file. trak reads no
/// list, but a future key might, and the cap is what stops a file that opens
/// brackets forever from driving this onto the stack.
const MAX_DEPTH: usize = 8;

/// Where the config lives, decided by two environment variables and nothing
/// else.
///
/// Injectable rather than read from `std::env` inside the loader, for two
/// reasons: a test has to be able to say "your `HOME` is this temporary
/// directory" without touching the real one, and `art.rs`'s
/// `XDG_CACHE_HOME`-from-the-environment approach would make every test in the
/// crate race over one process-wide variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    dir: PathBuf,
}

impl Paths {
    /// The real paths, from the process's own environment.
    pub fn from_env() -> Self {
        Self::from_vars(
            std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
            std::env::var_os("HOME").map(PathBuf::from),
        )
    }

    /// From the two variables SPEC §8 names, `XDG_CONFIG_HOME` first because it
    /// wins when it is set.
    ///
    /// With neither, the path is `.config/trak` relative to the working
    /// directory, which fails as an ordinary "could not read" rather than
    /// panicking: a trak with no `HOME` is odd, not a crash.
    pub fn from_vars(xdg_config_home: Option<PathBuf>, home: Option<PathBuf>) -> Self {
        let base = match xdg_config_home {
            Some(dir) => dir,
            None => home.unwrap_or_default().join(".config"),
        };
        Self {
            dir: base.join(DIR_NAME),
        }
    }

    /// The directory the file lives in.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The config file.
    pub fn file(&self) -> PathBuf {
        self.dir.join(FILE_NAME)
    }

    /// Where a file that could not be parsed is moved to. Not the same as
    /// [`Paths::dir`], and derived rather than stored so it cannot disagree.
    pub fn backup(&self) -> PathBuf {
        self.dir.join(format!("{FILE_NAME}{BACKUP_SUFFIX}"))
    }
}

/// A file that is there and is not a config file.
///
/// Every one of these ends at the defaults plus a moved-aside file rather than at
/// a refusal, because a config a person edited must never be the reason trak will
/// not start. [`Loaded::recovered`] is where that outcome is recorded;
/// [`ConfigError`] is for the failures that are about the disk rather than about
/// the contents.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// A line that is not a comment, a table header, or `key = value`.
    #[error("trak: config line {line}: {reason}")]
    Line { line: usize, reason: &'static str },
    /// Not UTF-8. Checked up front rather than per line, because a file of bytes
    /// that are not text is not a config under any reading.
    #[error("trak: config is not text")]
    NotText,
    /// Larger than [`MAX_BYTES`].
    #[error("trak: config is {bytes} bytes")]
    TooLarge { bytes: u64 },
}

impl ParseError {
    /// One line, for a toast. The line number is in it because "your config is
    /// broken" is not something anybody can act on.
    pub fn notice(&self) -> String {
        self.to_string()
    }
}

/// A disk failure around the config file, as opposed to a broken file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// The file is there and cannot be read: wrong permissions, or a directory
    /// where the file should be. A missing file is not this -- it is defaults.
    #[error("trak: could not read {path}")]
    Read { path: PathBuf },
    /// The directory that should hold the file is not there and could not be
    /// made. On a save this is the first-run path, so it only reaches a toast
    /// when something else is wrong with the disk.
    #[error("trak: could not create {path}")]
    Mkdir { path: PathBuf },
    /// The temp file, the write, or the rename that publishes it failed.
    #[error("trak: could not write {path}")]
    Write { path: PathBuf },
}

impl ConfigError {
    /// One line, for a toast. Never a stack trace and never a panic.
    pub fn notice(&self) -> String {
        self.to_string()
    }
}

/// What a load found, and what it did about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    /// Always usable: defaults wherever the file was silent, wrong, or broken.
    pub config: Config,
    /// The file that was read, whether or not there was one.
    pub path: PathBuf,
    /// What happened to a file that could not be parsed.
    pub recovered: Recovered,
}

impl Loaded {
    fn new(config: Config, path: &Path, recovered: Recovered) -> Self {
        Self {
            config,
            path: path.to_path_buf(),
            recovered,
        }
    }

    /// One line for a toast, and nothing at all when the file was fine. A load
    /// that did what it should says nothing.
    pub fn notice(&self) -> Option<String> {
        match &self.recovered {
            Recovered::None => None,
            Recovered::Moved { backup, reason } => {
                let name = backup.file_name().unwrap_or_default().to_string_lossy();
                Some(format!("{reason}; the old file is now {name}"))
            }
            Recovered::Kept { reason } => Some(format!(
                "{reason}; it could not be set aside, so a save will overwrite it"
            )),
        }
    }
}

/// What became of a file that could not be parsed.
///
/// A value rather than an error, because the answer is the defaults and those
/// are a perfectly good state: what a caller needs to say about it is one line,
/// which [`Loaded::notice`] builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovered {
    /// No file, or a file that parsed. Nothing to say.
    None,
    /// Moved to this path and the defaults are in use.
    Moved { backup: PathBuf, reason: String },
    /// Could not be moved -- so it is still where it was, and the defaults are in
    /// use over the top of it. A save will overwrite it.
    Kept { reason: String },
}

/// `[display]` -- what the interface shows, and how it is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Display {
    /// Whether to fetch and draw the cover at all. Ignored while `mode` is the
    /// visualizer, which draws over the same rectangle.
    pub art: bool,
    pub mode: DisplayMode,
    /// The progress bar under the title block.
    pub progress: bool,
    /// The `▰▱` meter. This is about *showing* the volume; which volume the
    /// volume keys change is [`Volume::control`].
    pub volume: bool,
    /// The popularity row on the Info tab.
    pub popularity: bool,
    /// The footer hints.
    pub key_hints: bool,
    pub clock: bool,
    pub side_pane: bool,
    /// Which tab opens on launch. Version A's tabs are not in this build yet.
    pub default_tab: Tab,
    pub border: Border,
    pub accent: Accent,
    /// Which terminal graphics protocol the cover is drawn with.
    pub art_protocol: ArtProtocol,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            art: true,
            mode: DisplayMode::Art,
            progress: true,
            volume: true,
            popularity: true,
            key_hints: true,
            clock: true,
            side_pane: true,
            default_tab: Tab::History,
            border: Border::Rounded,
            accent: Accent::Art,
            art_protocol: ArtProtocol::Auto,
        }
    }
}

/// `[visualizer]` -- what the visualizer looks like and where its bars come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visualizer {
    pub style: VisualizerStyle,
    pub source: VisualizerSource,
}

impl Default for Visualizer {
    fn default() -> Self {
        Self {
            style: VisualizerStyle::Spectrum,
            source: VisualizerSource::Auto,
        }
    }
}

/// `[input]` -- what a key press does, not where the pointer goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Input {
    /// When off, the loop does not even ask the terminal for mouse events.
    pub mouse: bool,
    /// One press of `+` or `-` moves the volume by this many points, so it is a
    /// step and not a percentage: the range is 0-100 either way.
    pub volume_step: i16,
    /// One press of `,` or `.` seeks by this many seconds. A fraction is
    /// allowed and written as one; whole numbers are written without the point,
    /// which is how a person would have typed them.
    pub seek_step: f64,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            mouse: true,
            volume_step: 10,
            seek_step: 5.0,
        }
    }
}

/// `[notifications]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Notifications {
    /// Off by default (TODO 4.5). A notification on every track change is a
    /// notification that gets switched off in week one.
    pub song_change: bool,
}

/// `[lyrics]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lyrics {
    pub enabled: bool,
}

impl Default for Lyrics {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// `[spotify]` -- the Web API (SPEC §6). Version B never needs any of it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Spotify {
    /// Empty means Version B: no login, no search, no queue. Set by the guided
    /// flow in TODO 5.3, which is also where it is validated -- a Client ID that
    /// is the wrong shape is worth refusing at the point it is pasted, not on
    /// every later request.
    pub client_id: String,
}

/// `[volume]` -- which volume the volume keys change (TODO 4.4, R2).
///
/// Not in SPEC §8's example, which is the only section of that file this module
/// does not follow: 4.4 pins the key as `volume.control`, `Settings` carries the
/// choice, and a config layer that quietly dropped it would make the setting
/// unreachable from disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    pub control: VolumeControl,
}

impl Default for Volume {
    fn default() -> Self {
        Self {
            control: VolumeControl::Spotify,
        }
    }
}

/// Every key SPEC §8 defines, as the file's own sections.
///
/// Deriving it is not a shortcut around SPEC §8: every default here is its
/// section's, and each section writes its own out where a reader of this file can
/// find it next to the fields it belongs to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Config {
    pub display: Display,
    pub visualizer: Visualizer,
    pub input: Input,
    pub notifications: Notifications,
    pub lyrics: Lyrics,
    pub spotify: Spotify,
    pub volume: Volume,
}

/// How the cover is drawn, and which terminal can be asked for it (SPEC §7).
///
/// Defined here rather than in `art.rs` because TODO 1.4 and 4.2 carry the
/// protocol nowhere: it exists as a `ratatui-image` query at the point of use and
/// has no type of its own yet. If `art.rs` grows one, this is the type to
/// replace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtProtocol {
    /// Ask the terminal, which is the only way to get the right answer for it.
    Auto,
    Kitty,
    Iterm2,
    Sixel,
    /// Unicode half blocks: the only thing Terminal.app does, and the fallback
    /// every terminal can do.
    HalfBlocks,
}

impl ArtProtocol {
    pub const ALL: [ArtProtocol; 5] = [
        ArtProtocol::Auto,
        ArtProtocol::Kitty,
        ArtProtocol::Iterm2,
        ArtProtocol::Sixel,
        ArtProtocol::HalfBlocks,
    ];

    pub fn parse(s: &str) -> Option<Self> {
        let wanted = s.trim();
        Self::ALL.into_iter().find(|p| p.label() == wanted)
    }

    pub fn label(self) -> &'static str {
        match self {
            ArtProtocol::Auto => "auto",
            ArtProtocol::Kitty => "kitty",
            ArtProtocol::Iterm2 => "iterm2",
            ArtProtocol::Sixel => "sixel",
            ArtProtocol::HalfBlocks => "halfblocks",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|p| *p == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// Where the visualizer's bars come from (SPEC §7).
///
/// `Auto` is the real-audio tap with the simulated bars as its fallback, and the
/// fallback is not a degraded mode: a permission the owner did not grant should
/// still give them something that moves with the music.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualizerSource {
    Auto,
    /// Force the stand-in. Useful where the tap cannot work at all, and the only
    /// honest way to test the renderer without an audio device.
    Simulated,
}

impl VisualizerSource {
    pub const ALL: [VisualizerSource; 2] = [VisualizerSource::Auto, VisualizerSource::Simulated];

    pub fn parse(s: &str) -> Option<Self> {
        let wanted = s.trim();
        Self::ALL.into_iter().find(|s| s.label() == wanted)
    }

    pub fn label(self) -> &'static str {
        match self {
            VisualizerSource::Auto => "auto",
            VisualizerSource::Simulated => "simulated",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// The name `default_tab` is written under, for the tab `tab`.
///
/// Public because Version A adds tabs (SPEC §6) and this is the one place that
/// has to learn their names.
pub fn tab_name(tab: Tab) -> &'static str {
    match tab {
        Tab::Search => "search",
        Tab::Playlists => "playlists",
        Tab::Queue => "queue",
        Tab::Liked => "liked",
        Tab::Library => "library",
        Tab::Lyrics => "lyrics",
        Tab::History => "history",
        Tab::Info => "info",
    }
}

/// A tab from the name written under `default_tab`, or `None` for a word that is
/// not one.
///
/// The Version A names are the five `Tab::VERSION_A` variants, and they are
/// accepted: a config written on a build that had them must still open on one
/// that does not, or the setting would silently reset.
pub fn tab_from_config(name: &str) -> Option<Tab> {
    let wanted = name.trim();
    Tab::ALL.into_iter().find(|t| tab_name(*t) == wanted)
}

impl Config {
    /// The settings the TUI reads, with this file applied.
    ///
    /// Built from [`Settings::default`] rather than from whatever the running app
    /// holds, so the keys TODO 5.3 has not given a field to are at their defaults
    /// instead of at a value from an earlier session.
    pub fn settings(&self) -> Settings {
        let mut settings = Settings::default();
        self.apply(&mut settings);
        settings
    }

    /// Copy the keys both sides have into `settings`, and leave the rest alone.
    ///
    /// `art`, `progress`, `popularity`, `default_tab` and `art_protocol` have no
    /// field on `Settings` yet; TODO 5.3 is what gives them one, and until then
    /// this leaves them alone rather than guessing.
    pub fn apply(&self, settings: &mut Settings) {
        settings.seek_step = self.input.seek_step;
        settings.volume_step = self.input.volume_step;
        settings.mouse = self.input.mouse;
        settings.show_clock = self.display.clock;
        settings.show_volume = self.display.volume;
        settings.show_key_hints = self.display.key_hints;
        settings.side_pane = self.display.side_pane;
        settings.lyrics = self.lyrics.enabled;
        settings.song_change_notification = self.notifications.song_change;
        settings.volume_control = self.volume.control;
        settings.display_mode = self.display.mode;
        settings.visualizer_style = self.visualizer.style;
        settings.show_art = self.display.art;
        settings.border = self.display.border;
        settings.accent = self.display.accent;
        settings.art_protocol = self.display.art_protocol;
        settings.show_progress = self.display.progress;
        settings.show_popularity = self.display.popularity;
        settings.default_tab = self.display.default_tab;
        settings.visualizer_source = self.visualizer.source;
    }

    /// This config with the keys [`Settings`] has read back out of it, which is
    /// how the settings screen (TODO 5.2) saves: start from what was loaded and
    /// replace only what the screen can change, so a key it does not know about
    /// survives a save.
    ///
    pub fn with_settings(&self, settings: &Settings) -> Config {
        let mut next = self.clone();
        next.input.seek_step = settings.seek_step;
        next.input.volume_step = settings.volume_step;
        next.input.mouse = settings.mouse;
        next.display.clock = settings.show_clock;
        next.display.volume = settings.show_volume;
        next.display.key_hints = settings.show_key_hints;
        next.display.side_pane = settings.side_pane;
        next.lyrics.enabled = settings.lyrics;
        next.notifications.song_change = settings.song_change_notification;
        next.volume.control = settings.volume_control;
        next.display.mode = settings.display_mode;
        next.visualizer.style = settings.visualizer_style;
        next.display.art = settings.show_art;
        next.display.border = settings.border;
        next.display.accent = settings.accent;
        next.display.art_protocol = settings.art_protocol;
        next.display.progress = settings.show_progress;
        next.display.popularity = settings.show_popularity;
        next.display.default_tab = settings.default_tab;
        next.visualizer.source = settings.visualizer_source;
        next
    }

    /// The file `save` would write, in the shape and key order of SPEC §8 with
    /// its comments.
    ///
    /// Pure, and worth testing on its own: the comments and the alignment are
    /// for a person reading the file, and a key written out of order or under a
    /// name nothing reads is invisible until somebody edits the wrong line.
    pub fn to_toml(&self) -> String {
        let mut out = String::new();
        out.push_str(HEADER);
        table(&mut out, "display");
        kv(
            &mut out,
            "art",
            self.display.art,
            "show album art (ignored while the visualizer is showing)",
        );
        kv(
            &mut out,
            "mode",
            quoted(display_mode_name(self.display.mode)),
            "\"art\" | \"visualizer\"",
        );
        kv(&mut out, "progress", self.display.progress, "");
        kv(&mut out, "volume", self.display.volume, "");
        kv(&mut out, "popularity", self.display.popularity, "");
        kv(&mut out, "key_hints", self.display.key_hints, "");
        kv(&mut out, "clock", self.display.clock, "");
        kv(&mut out, "side_pane", self.display.side_pane, "");
        kv(
            &mut out,
            "default_tab",
            quoted(tab_name(self.display.default_tab)),
            "B: history|info|lyrics ; A: search|playlists|queue|liked|library|lyrics",
        );
        kv(
            &mut out,
            "border",
            quoted(border_name(self.display.border)),
            "rounded | sharp | double | none",
        );
        kv(
            &mut out,
            "accent",
            quoted(accent_name(self.display.accent)),
            "art | green | terminal",
        );
        kv(
            &mut out,
            "art_protocol",
            quoted(self.display.art_protocol.label()),
            "auto | kitty | iterm2 | sixel | halfblocks",
        );

        table(&mut out, "visualizer");
        kv(
            &mut out,
            "style",
            quoted(self.visualizer.style.label()),
            "spectrum | mirrored | waveform | circular",
        );
        kv(
            &mut out,
            "source",
            quoted(self.visualizer.source.label()),
            "auto | simulated",
        );

        table(&mut out, "input");
        kv(&mut out, "mouse", self.input.mouse, "");
        kv(&mut out, "volume_step", self.input.volume_step, "");
        kv(
            &mut out,
            "seek_step",
            seek_step_text(self.input.seek_step),
            "",
        );

        table(&mut out, "volume");
        kv(
            &mut out,
            "control",
            quoted(self.volume.control.label()),
            "spotify | system",
        );

        table(&mut out, "notifications");
        kv(&mut out, "song_change", self.notifications.song_change, "");

        table(&mut out, "lyrics");
        kv(&mut out, "enabled", self.lyrics.enabled, "");

        table(&mut out, "spotify");
        kv(
            &mut out,
            "client_id",
            quoted(&self.spotify.client_id),
            "empty = Version B",
        );
        out
    }

    /// Load from the real paths: `$XDG_CONFIG_HOME/trak/config.toml`, else
    /// `~/.config/trak/config.toml` (SPEC §8).
    pub fn load() -> Result<Loaded, ConfigError> {
        Self::load_from(&Paths::from_env().file())
    }

    /// Load a named file. The whole filesystem half of loading is this function,
    /// so the rest of the module is testable with bytes alone.
    pub fn load_from(path: &Path) -> Result<Loaded, ConfigError> {
        let Some(bytes) = read_file(path)? else {
            // No file is every default, which is the first-run path (TODO 5.4)
            // rather than a failure: most people never have this file.
            return Ok(Loaded::new(Config::default(), path, Recovered::None));
        };
        match parse(&bytes) {
            Ok(config) => Ok(Loaded::new(config, path, Recovered::None)),
            Err(reason) => {
                let reason = reason.to_string();
                let backup = backup_for(path);
                // A rename and not a copy-and-delete: it cannot be interrupted
                // half way, and it keeps the bytes exactly as they were. The
                // alternative -- refusing to start because the file is broken --
                // is not a trade anybody wants.
                let recovered = match fs::rename(path, &backup) {
                    Ok(()) => Recovered::Moved { backup, reason },
                    // Kept rather than deleted: the file may be the only copy of
                    // something the owner wants, and the defaults are in use over
                    // the top of it either way.
                    Err(_) => Recovered::Kept { reason },
                };
                Ok(Loaded::new(Config::default(), path, recovered))
            }
        }
    }

    /// Save to the real paths, atomically, with mode `0600`.
    pub fn save(&self) -> Result<(), ConfigError> {
        self.save_to(&Paths::from_env().file())
    }

    /// Save to a named file: a temp file beside it and a rename over it.
    ///
    /// Atomic because a half-written config is not a config: it would be parsed
    /// as corrupt on the next start, and by the corrupt-file rule that means every
    /// setting silently reset to its default. The temp file is a sibling rather
    /// than somewhere in `$TMPDIR` because a rename across filesystems is a copy,
    /// and a copy is exactly what this is here to avoid.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let dir = dir_of(path);
        if !dir.is_dir() {
            fs::create_dir_all(dir).map_err(|_| ConfigError::Mkdir {
                path: dir.to_path_buf(),
            })?;
        }
        let temp = temp_path(path);
        write_private(&temp, self.to_toml().as_bytes())?;
        fs::rename(&temp, path).map_err(|_| ConfigError::Write {
            path: path.to_path_buf(),
        })
    }
}

/// The header of a file `save` writes: three lines saying what may safely be
/// done to it. A hand-edited config is a supported way to use trak (SPEC §8), so
/// the file has to say what happens when it is wrong.
const HEADER: &str = "\
# trak config. Written by `trak config` and by `,` inside the TUI, and meant to
# be edited by hand: unknown keys are ignored, a missing key takes its default,
# and a file that cannot be read is moved to config.toml.bak rather than lost.
";

/// The comment column SPEC §8 puts comments in, so a file trak writes and one
/// copied out of the spec look the same.
const COMMENT_COLUMN: usize = 27;

/// One `[name]` header, preceded by the blank line that separates sections.
fn table(out: &mut String, name: &str) {
    out.push('\n');
    out.push('[');
    out.push_str(name);
    out.push_str("]\n");
}

/// The name `mode` is written under: the inverse of [`DisplayMode::parse`], which
/// stays with the type so the schema and the renderer cannot disagree. A new
/// variant makes this a compile error rather than a value a file cannot express.
fn display_mode_name(mode: DisplayMode) -> &'static str {
    match mode {
        DisplayMode::Art => "art",
        DisplayMode::Visualizer => "visualizer",
    }
}

/// The inverse of [`Border::parse`].
fn border_name(border: Border) -> &'static str {
    match border {
        Border::Rounded => "rounded",
        Border::Sharp => "sharp",
        Border::Double => "double",
        Border::None => "none",
    }
}

/// The inverse of [`Accent::parse`].
fn accent_name(accent: Accent) -> &'static str {
    match accent {
        Accent::Art => "art",
        Accent::Green => "green",
        Accent::Terminal => "terminal",
    }
}

/// A seek step as a person would write it: `5`, not `5.0`. A real fraction keeps
/// its point, because dropping one would change the value.
fn seek_step_text(step: f64) -> String {
    if step.is_finite() && step.fract() == 0.0 {
        format!("{}", step as i64)
    } else {
        format!("{step}")
    }
}

/// One `key = value` line, with its comment lined up in [`COMMENT_COLUMN`].
fn kv(out: &mut String, key: &str, value: impl std::fmt::Display, comment: &str) {
    let line = format!("{key} = {value}");
    out.push_str(&line);
    if !comment.is_empty() {
        // At least one space, so a value longer than the comment column (a
        // 32-character Client ID, say) does not come out as `..."# comment`.
        for _ in line.len().min(COMMENT_COLUMN - 1)..COMMENT_COLUMN {
            out.push(' ');
        }
        out.push_str("# ");
        out.push_str(comment);
    }
    out.push('\n');
}

/// A `table = value` string, as a TOML basic string.
///
/// `client_id` is the only value a person can type freely, and a quote or a
/// backslash in it must not produce a file that reads as corrupt.
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Everything else that cannot be written literally, so the file stays
            // one line per key whatever the string holds.
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The directory a save has to create before it can write.
///
/// A bare file name has an empty parent, and the empty path is not something that
/// can be created -- the current directory is what a caller naming one means.
fn dir_of(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// The file name of `path`, or [`FILE_NAME`] for a path that has none, so a caller
/// naming a directory rather than a file cannot make this write somewhere odd.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| FILE_NAME.to_string())
}

/// Where a broken file at `path` goes.
fn backup_for(path: &Path) -> PathBuf {
    path.with_file_name(format!("{}{BACKUP_SUFFIX}", name_of(path)))
}

/// A sibling of the file being written, so the rename publishes it atomically.
fn temp_path(path: &Path) -> PathBuf {
    path.with_file_name(format!(".{}.tmp", name_of(path)))
}

/// Read a config file, or `None` for "there is not one".
///
/// Bounded by [`MAX_BYTES`] rather than trusted to be small: `parse` refuses that
/// size anyway, and this is where a file that is not a config stops being read.
fn read_file(path: &Path) -> Result<Option<Vec<u8>>, ConfigError> {
    let file = match File::open(path) {
        Ok(file) => file,
        // A file that is not there is the first-run path, not an error, and the
        // only absence that is not worth a line.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(ConfigError::Read {
                path: path.to_path_buf(),
            });
        }
    };
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ConfigError::Read {
            path: path.to_path_buf(),
        })?;
    Ok(Some(bytes))
}

/// Write `bytes` to `path`, owner-only, and flushed before the caller renames it.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let wrote = |_| ConfigError::Write {
        path: path.to_path_buf(),
    };
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        // The mode at creation, and then set again below, because either alone is
        // not enough: the creation mode is masked by the umask, and a temp file
        // left behind by a killed trak is a file that already exists with
        // whatever mode it was last written with.
        .mode(0o600)
        .open(path)
        .map_err(wrote)?;
    file.write_all(bytes).map_err(wrote)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(wrote)?;
    // Flushed before the rename, or the rename can land with the contents still in
    // the buffer -- a zero-length config, and by the corrupt-file rule a reset.
    file.sync_all().map_err(wrote)
}

/// Every key in one file, parsed.
///
/// The only entry point into the reader, so `Config::load_from` and the tests
/// agree on what a corrupt file is.
pub fn parse(bytes: &[u8]) -> Result<Config, ParseError> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ParseError::TooLarge {
            bytes: bytes.len() as u64,
        });
    }
    // A byte-order mark is stepped over rather than refused: TextEdit writes one
    // and a person editing this file may well be using it. Refusing would mean the
    // file is moved aside and every setting in it is lost to three invisible bytes.
    let text = std::str::from_utf8(bytes).map_err(|_| ParseError::NotText)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let entries = Reader::new(text.as_bytes()).document()?;
    Ok(Config::from_entries(&entries))
}

/// One `key = value` line, with the table it was written under.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    table: String,
    key: String,
    value: Value,
}

/// A parsed value.
///
/// Only the kinds the schema uses are read. The other two are here so that a key
/// from a future trak -- a list, a date, a float -- is stepped over rather than
/// refused as a broken file, which is the whole point of ignoring unknown keys.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    /// A list, of any shape trak does not read. Never read, only skipped.
    List(Vec<Value>),
    /// Any other bare token: a date, a time, `inf`.
    Other(String),
}

impl Value {
    /// The first `key = value` written under `table`, if there is one.
    ///
    /// First wins for a repeated key. TOML calls that an error; a config read
    /// twice by hand is more likely to be a person appending to a section than
    /// anything else, and taking the first is the reading that keeps the top of
    /// the file -- the part somebody is looking at -- the one that counts.
    fn lookup<'a>(entries: &'a [Entry], table: &str, key: &str) -> Option<&'a Value> {
        entries
            .iter()
            .find(|e| e.table == table && e.key == key)
            .map(|e| &e.value)
    }
}

impl Config {
    /// Build from what the file said, starting at every default.
    ///
    /// One line per key, and every assignment goes through a `set_` below that
    /// acts only on the one kind of value the key can hold. That is the whole of
    /// the wrong-type rule: `volume = "loud"` leaves the default in place and
    /// leaves every other key alone, because a bad value never reaches further
    /// than the one field it belongs to.
    fn from_entries(entries: &[Entry]) -> Self {
        let get = |table: &str, key: &str| Value::lookup(entries, table, key);
        let mut config = Config::default();
        set_bool(&mut config.display.art, get("display", "art"));
        set_enum(
            &mut config.display.mode,
            get("display", "mode"),
            DisplayMode::parse,
        );
        set_bool(&mut config.display.progress, get("display", "progress"));
        set_bool(&mut config.display.volume, get("display", "volume"));
        set_bool(&mut config.display.popularity, get("display", "popularity"));
        set_bool(&mut config.display.key_hints, get("display", "key_hints"));
        set_bool(&mut config.display.clock, get("display", "clock"));
        set_bool(&mut config.display.side_pane, get("display", "side_pane"));
        set_enum(
            &mut config.display.default_tab,
            get("display", "default_tab"),
            tab_from_config,
        );
        set_enum(
            &mut config.display.border,
            get("display", "border"),
            Border::parse,
        );
        set_enum(
            &mut config.display.accent,
            get("display", "accent"),
            Accent::parse,
        );
        set_enum(
            &mut config.display.art_protocol,
            get("display", "art_protocol"),
            ArtProtocol::parse,
        );
        set_enum(
            &mut config.visualizer.style,
            get("visualizer", "style"),
            VisualizerStyle::parse,
        );
        set_enum(
            &mut config.visualizer.source,
            get("visualizer", "source"),
            VisualizerSource::parse,
        );
        set_bool(&mut config.input.mouse, get("input", "mouse"));
        set_step(&mut config.input.volume_step, get("input", "volume_step"));
        set_number(&mut config.input.seek_step, get("input", "seek_step"));
        set_enum(
            &mut config.volume.control,
            get("volume", "control"),
            VolumeControl::parse,
        );
        set_bool(
            &mut config.notifications.song_change,
            get("notifications", "song_change"),
        );
        set_bool(&mut config.lyrics.enabled, get("lyrics", "enabled"));
        set_text(&mut config.spotify.client_id, get("spotify", "client_id"));
        config
    }
}

/// A boolean, or the default left alone.
fn set_bool(slot: &mut bool, value: Option<&Value>) {
    if let Some(Value::Bool(found)) = value {
        *slot = *found;
    }
}

/// A word from `parse`, or the default left alone. An unknown word is not a
/// broken file either: a value from a future trak must not reset this one.
fn set_enum<T>(slot: &mut T, value: Option<&Value>, parse: fn(&str) -> Option<T>) {
    if let Some(Value::Str(text)) = value
        && let Some(found) = parse(text)
    {
        *slot = found;
    }
}

/// A whole number, if it is one and fits. `volume_step = 300` is out of range
/// rather than corrupt, and takes the default.
fn set_step(slot: &mut i16, value: Option<&Value>) {
    if let Some(Value::Int(found)) = value
        && let Ok(found) = i16::try_from(*found)
    {
        *slot = found;
    }
}

/// A number for a field that is a number. `5` and `5.0` are both five, because a
/// hand-edited file will have both and neither is a mistake worth a default.
fn set_number(slot: &mut f64, value: Option<&Value>) {
    match value {
        Some(Value::Int(found)) => *slot = *found as f64,
        Some(Value::Float(found)) => *slot = *found,
        _ => {}
    }
}

/// A string, including the empty one: `client_id = ""` is what Version B writes
/// and reads back as itself.
fn set_text(slot: &mut String, value: Option<&Value>) {
    if let Some(Value::Str(found)) = value {
        *slot = found.clone();
    }
}

/// Just enough TOML for a file of tables of scalars.
///
/// Liberal about whitespace, comments, quoting and about values trak does not
/// read, and strict about the two things a person can get wrong in a way that
/// changes the meaning of the rest of the file: a string that never closes and a
/// key with no `=`. Everything else is a skipped key rather than a refusal.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    /// 1-based, so a message points at the line a person is looking at.
    line: usize,
    depth: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            at: 0,
            line: 1,
            depth: 0,
        }
    }

    /// Every `key = value` in the document, with the table each was written
    /// under. A file with no table header at all puts its keys in the root table,
    /// which is not one trak reads -- they are ignored like any other unknown key.
    fn document(&mut self) -> Result<Vec<Entry>, ParseError> {
        let mut entries = Vec::new();
        let mut table = String::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                None => return Ok(entries),
                Some(b'[') => table = self.table_header()?,
                _ => entries.push(self.pair(&table)?),
            }
        }
    }

    /// `[name]`, and the name is taken as written: `[a.b]` or `[display]` are both
    /// just names, and the second one only means something because it is spelled
    /// the way a table is.
    fn table_header(&mut self) -> Result<String, ParseError> {
        self.bump();
        self.skip_spaces();
        let name = self.key()?;
        self.skip_spaces();
        if self.bump() != Some(b']') {
            return Err(self.err("a table header needs a `]`"));
        }
        self.finish_line()?;
        Ok(name)
    }

    /// `key = value`, and nothing else on the line but a comment. Two pairs on one
    /// line is refused rather than guessed at: which of them the person meant is
    /// not answerable, and reading the first would quietly pick one.
    fn pair(&mut self, table: &str) -> Result<Entry, ParseError> {
        let key = self.key()?;
        self.skip_spaces();
        if self.bump() != Some(b'=') {
            return Err(self.err("a key needs an `=`"));
        }
        self.skip_spaces();
        let value = self.value()?;
        self.finish_line()?;
        Ok(Entry {
            table: table.to_string(),
            key,
            value,
        })
    }

    /// A table name or a key: quoted, or bare.
    fn key(&mut self) -> Result<String, ParseError> {
        match self.peek() {
            Some(b'"') => self.string(),
            Some(b'\'') => self.literal_string(),
            _ => {
                let bare = self.token();
                if bare.is_empty() {
                    return Err(self.err("expected a key"));
                }
                Ok(bare)
            }
        }
    }

    fn value(&mut self) -> Result<Value, ParseError> {
        match self.peek() {
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b'\'') => Ok(Value::Str(self.literal_string()?)),
            Some(b'[') => self.list(),
            Some(b'{') => self.inline_table(),
            _ => {
                let token = self.token();
                if token.is_empty() {
                    return Err(self.err("a key needs a value"));
                }
                Ok(classify(&token))
            }
        }
    }

    /// An inline table, which trak reads in no key. Stepped over rather than
    /// parsed: the inside is only meaningful to whatever key a future trak adds,
    /// and refusing it would make that file a corrupt one -- forty good settings
    /// lost to a line trak has no use for.
    fn inline_table(&mut self) -> Result<Value, ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.err("tables nested too deep"));
        }
        let start = self.at;
        self.bump();
        let mut open = 1usize;
        loop {
            match self.bump() {
                None => return Err(self.err("a table needs a `}`")),
                Some(b'{') => open += 1,
                Some(b'}') => {
                    open -= 1;
                    if open == 0 {
                        break;
                    }
                }
                // A quoted run inside is stepped over with its own brackets, so a
                // `}` in a string cannot close the table early.
                Some(b'"' | b'\'') => self.skip_quoted()?,
                _ => {}
            }
        }
        self.depth -= 1;
        Ok(Value::Other(
            String::from_utf8_lossy(&self.bytes[start..self.at]).into_owned(),
        ))
    }

    /// Step over a quoted run without interpreting it, for a value that is not one
    /// trak reads. An escape is honoured inside `"` because that is what keeps a
    /// `\"` from ending the run early; a `'` string has no escapes, so a backslash
    /// in one is an ordinary character.
    fn skip_quoted(&mut self) -> Result<(), ParseError> {
        let opened = self.line;
        let quote = self.bump();
        while let Some(byte) = self.bump() {
            if quote == Some(b'"') && byte == b'\\' {
                self.bump();
                continue;
            }
            if Some(byte) == quote {
                return Ok(());
            }
        }
        Err(self.err_at(opened, "a string needs a closing quote"))
    }

    /// A basic string, with TOML's escapes. A newline inside one is refused: it
    /// is the shape a half-written file has, and reading it as a string would
    /// swallow the rest of the file.
    fn string(&mut self) -> Result<String, ParseError> {
        // The line the quote opened on, because a string that runs to the end of
        // the file has moved the line on by then and the message has to point at
        // where the value started.
        let opened = self.line;
        self.bump();
        let mut out: Vec<u8> = Vec::new();
        loop {
            match self.bump() {
                Some(b'"') => break,
                Some(b'\\') => self.escape(&mut out)?,
                // A raw newline or the end of the file: the string never closed.
                Some(b'\n') | None => {
                    return Err(self.err_at(opened, "a string needs a closing quote"));
                }
                Some(byte) => out.push(byte),
            }
        }
        // The file was checked as UTF-8 before any of this ran, so the only bytes
        // here are the ones it already accepted. Checked anyway, because a panic
        // on somebody's config file is never acceptable.
        String::from_utf8(out).map_err(|_| ParseError::NotText)
    }

    /// A literal string: no escapes at all, which is what somebody means when
    /// they write a Windows path or a regex with a backslash in it.
    fn literal_string(&mut self) -> Result<String, ParseError> {
        let opened = self.line;
        self.bump();
        let start = self.at;
        loop {
            match self.bump() {
                Some(b'\'') => break,
                Some(b'\n') | None => {
                    return Err(self.err_at(opened, "a string needs a closing quote"));
                }
                Some(_) => {}
            }
        }
        // Only ASCII delimiters were stepped over, so the slice is UTF-8.
        Ok(String::from_utf8_lossy(&self.bytes[start..self.at - 1]).into_owned())
    }

    fn escape(&mut self, out: &mut Vec<u8>) -> Result<(), ParseError> {
        let found = match self.bump() {
            Some(found) => found,
            None => return Err(self.err("an escape needs a letter")),
        };
        let ch = match found {
            b'"' => '"',
            b'\\' => '\\',
            b'b' => '\u{8}',
            b'f' => '\u{c}',
            b'n' => '\n',
            b'r' => '\r',
            b't' => '\t',
            b'u' => self.unicode_escape()?,
            // Refused rather than passed through: an unknown escape means the file
            // was written by something that is not this reader, and reading it
            // anyway would mean guessing what it meant.
            _ => return Err(self.err("an unknown escape")),
        };
        let mut buf = [0u8; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        Ok(())
    }

    /// `\uXXXX`, and only that form: `\UXXXXXXXX` and the braced escapes are
    /// refused, since nothing trak writes or reads carries one.
    fn unicode_escape(&mut self) -> Result<char, ParseError> {
        let mut unit: u16 = 0;
        for _ in 0..4 {
            let byte = match self.bump() {
                Some(byte) => byte,
                None => return Err(self.err("an escape needs four hex digits")),
            };
            let digit = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(self.err("an escape needs four hex digits")),
            };
            unit = unit * 16 + u16::from(digit);
        }
        // A surrogate or a unit that is not a character has no character behind
        // it, and picking one would turn somebody's string into replacement
        // characters on the way back out.
        char::from_u32(u32::from(unit)).ok_or_else(|| self.err("an escape is not a character"))
    }

    /// A list, which trak reads in no key. Parsed so that a future key with one
    /// is skipped rather than refused, and so that the brackets cannot run away.
    fn list(&mut self) -> Result<Value, ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.err("lists nested too deep"));
        }
        self.bump();
        let mut items = Vec::new();
        loop {
            self.skip_trivia();
            match self.peek() {
                None => return Err(self.err("a list needs a `]`")),
                Some(b']') => {
                    self.bump();
                    break;
                }
                // A trailing comma is legal TOML and is here because
                // list-per-line is how people write lists.
                Some(b',') => {
                    self.bump();
                }
                _ => items.push(self.value()?),
            }
        }
        self.depth -= 1;
        Ok(Value::List(items))
    }

    /// A bare token: everything up to whitespace, a comment or a separator.
    ///
    /// Deliberately wide rather than strict about what a token may contain -- a
    /// date, a duration, an enum word from a future trak all have to be
    /// *something* the reader can step over. What is left over is classified as
    /// [`Value::Other`], and an unknown key holding one is ignored like any other.
    fn token(&mut self) -> String {
        let start = self.at;
        while let Some(byte) = self.peek() {
            if matches!(
                byte,
                b' ' | b'\t' | b'\n' | b'\r' | b'#' | b',' | b']' | b'=' | b'"' | b'\''
            ) {
                break;
            }
            self.bump();
        }
        // The whole file was checked as UTF-8 and this slice is part of it, so
        // the lossy conversion can only ever copy.
        String::from_utf8_lossy(&self.bytes[start..self.at]).into_owned()
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.bump();
        }
    }

    /// Whitespace, newlines and comments: everything that carries no value.
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\n' | b'\r') => {
                    self.bump();
                }
                Some(b'#') => self.skip_comment(),
                _ => return,
            }
        }
    }

    fn skip_comment(&mut self) {
        while !matches!(self.peek(), None | Some(b'\n')) {
            self.bump();
        }
    }

    /// Only a comment may follow a value; the line ends at the newline.
    fn finish_line(&mut self) -> Result<(), ParseError> {
        loop {
            match self.peek() {
                Some(b' ' | b'\t') => {
                    self.bump();
                }
                Some(b'#') => self.skip_comment(),
                Some(b'\n' | b'\r') | None => return Ok(()),
                Some(_) => return Err(self.err("only a comment may follow a value")),
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek()?;
        self.at += 1;
        if byte == b'\n' {
            self.line += 1;
        }
        Some(byte)
    }

    fn err(&self, reason: &'static str) -> ParseError {
        self.err_at(self.line, reason)
    }

    /// A failure reported at the line something started on rather than where the
    /// reader happened to be when it noticed.
    fn err_at(&self, line: usize, reason: &'static str) -> ParseError {
        ParseError::Line { line, reason }
    }
}

/// What a bare token turned out to be.
fn classify(token: &str) -> Value {
    match token {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    // `_` separators are legal TOML and one will turn up in a large number typed
    // by a person, so they are allowed here rather than making the file corrupt.
    if is_integer(token)
        && let Ok(found) = token.replace('_', "").parse::<i64>()
    {
        return Value::Int(found);
    }
    if token.contains(['.', 'e', 'E'])
        && let Ok(found) = token.replace('_', "").parse::<f64>()
        && found.is_finite()
    {
        return Value::Float(found);
    }
    Value::Other(token.to_string())
}

/// Whether a token is an integer of a shape `i64` can hold.
fn is_integer(token: &str) -> bool {
    let body = token.strip_prefix(['+', '-']).unwrap_or(token);
    !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit() || b == b'_')
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// SPEC §8's file, verbatim, with the one section this module adds: TODO 4.4
    /// pins the setting as `volume.control`, `Settings` carries it, and a config
    /// layer that quietly dropped it would make it unreachable from disk. It is
    /// the only deviation from that block, and it is why two tests here assert on
    /// the whole of it rather than on the sections separately.
    const SPEC: &str = r#"[display]
art = true                 # show album art (ignored while the visualizer is showing)
mode = "art"               # "art" | "visualizer"
progress = true
volume = true
popularity = true
key_hints = true
clock = true
side_pane = true
default_tab = "history"    # B: history|info|lyrics ; A: search|playlists|queue|liked|library|lyrics
border = "rounded"         # rounded | sharp | double | none
accent = "art"             # art | green | terminal
art_protocol = "auto"      # auto | kitty | iterm2 | sixel | halfblocks

[visualizer]
style = "spectrum"         # spectrum | mirrored | waveform | circular
source = "auto"            # auto | simulated

[input]
mouse = true
volume_step = 10
seek_step = 5

[volume]
control = "spotify"        # spotify | system

[notifications]
song_change = false

[lyrics]
enabled = true

[spotify]
client_id = ""             # empty = Version B
"#;

    /// A temporary directory that removes itself. No tempfile dependency for one
    /// struct, and the name is per test because cargo runs them in threads of one
    /// process.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "trak-config-test-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A config directory that removes itself, and the [`Paths`] into it. Nothing
    /// here can reach the owner's own `~/.config/trak`, which is the whole point
    /// of the paths being injectable.
    fn sandbox(tag: &str) -> (TempDir, Paths) {
        let home = TempDir::new(tag);
        let paths = Paths::from_vars(None, Some(home.path().to_path_buf()));
        (home, paths)
    }

    /// Parse a string, failing the test rather than returning a default: a test
    /// that means to read a key must fail loudly if the file was not read at all.
    fn loaded(body: &str) -> Config {
        parse(body.as_bytes()).unwrap_or_else(|e| panic!("{body:?} should parse: {e}"))
    }

    /// Write a file into a directory and load it, so a test can go through the
    /// file layer without a whole config directory.
    fn load_file(dir: &Path, body: &str) -> Loaded {
        let path = dir.join(FILE_NAME);
        fs::create_dir_all(dir).expect("config dir");
        fs::write(&path, body).expect("write the test config");
        Config::load_from(&path).expect("load")
    }

    /// Every key at a non-default value, which is the only kind of config a
    /// round-trip test is worth anything with.
    fn everything() -> Config {
        Config {
            display: Display {
                art: false,
                mode: DisplayMode::Visualizer,
                progress: false,
                volume: false,
                popularity: false,
                key_hints: false,
                clock: false,
                side_pane: false,
                default_tab: Tab::Lyrics,
                border: Border::Double,
                accent: Accent::Green,
                art_protocol: ArtProtocol::Kitty,
            },
            visualizer: Visualizer {
                style: VisualizerStyle::Circular,
                source: VisualizerSource::Simulated,
            },
            input: Input {
                mouse: false,
                volume_step: 25,
                // A fraction on purpose: it is the one value that is written and
                // read in two different shapes, and a whole-number-only round trip
                // would not notice either being wrong.
                seek_step: 2.5,
            },
            notifications: Notifications { song_change: true },
            lyrics: Lyrics { enabled: false },
            spotify: Spotify {
                client_id: "4c2b19f7a1".into(),
            },
            volume: Volume {
                control: VolumeControl::System,
            },
        }
    }

    /// One key, on its own, in a file written by hand: it must read as exactly
    /// this and every other key as its default.
    ///
    /// The assertion is on the whole config rather than on the one field, so a key
    /// that is read and then dropped, or read into the wrong field, fails here.
    macro_rules! reads {
        ($name:ident, $file:literal, $expected:expr) => {
            #[test]
            fn $name() {
                let expected: Config = $expected;
                assert_eq!(loaded($file), expected, "{}", $file);
            }
        };
    }

    /// One key, on its own, in the file `save` writes: the line must be in it,
    /// and what was written must read back as what was set. Without the second
    /// half a key could be written under a name nothing reads and still pass.
    macro_rules! writes {
        ($name:ident, $config:expr, $line:literal) => {
            #[test]
            fn $name() {
                let config: Config = $config;
                let line = $line;
                let toml = config.to_toml();
                assert!(toml.contains(line), "{line:?} is missing from:\n{toml}");
                assert_eq!(loaded(&toml), config, "{line:?} did not read back");
            }
        };
    }

    // ---------------------------------------------------------------- [display]

    reads!(
        display_art_is_read,
        "[display]\nart = false\n",
        Config {
            display: Display {
                art: false,
                ..Display::default()
            },
            ..Config::default()
        }
    );
    writes!(
        display_art_is_written,
        Config {
            display: Display {
                art: false,
                ..Display::default()
            },
            ..Config::default()
        },
        "art = false"
    );

    reads!(
        display_mode_is_read,
        "[display]\nmode = \"visualizer\"\n",
        Config {
            display: Display {
                mode: DisplayMode::Visualizer,
                ..Display::default()
            },
            ..Config::default()
        }
    );
    writes!(
        display_mode_is_written,
        Config {
            display: Display {
                mode: DisplayMode::Visualizer,
                ..Display::default()
            },
            ..Config::default()
        },
        "mode = \"visualizer\""
    );

    reads!(
        display_progress_is_read,
        "[display]\nprogress = false\n",
        Config {
            display: Display {
                progress: false,
                ..Display::default()
            },
            ..Config::default()
        }
    );
    writes!(
        display_progress_is_written,
        Config {
            display: Display {
                progress: false,
                ..Display::default()
            },
            ..Config::default()
        },
        "progress = false"
    );

    reads!(
        display_volume_is_read,
        "[display]\nvolume = false\n",
        Config {
            display: Display {
                volume: false,
                ..Display::default()
            },
            ..Config::default()
        }
    );
    writes!(
        display_volume_is_written,
        Config {
            display: Display {
                volume: false,
                ..Display::default()
            },
            ..Config::default()
        },
        "volume = false"
    );

    reads!(
        display_popularity_is_read,
        "[display]\npopularity = false\n",
        Config {
            display: Display {
                popularity: false,
                ..Display::default()
            },
            ..Config::default()
        }
    );
    writes!(
        display_popularity_is_written,
        Config {
            display: Display {
                popularity: false,
                ..Display::default()
            },
            ..Config::default()
        },
        "popularity = false"
    );

    reads!(
        display_key_hints_is_read,
        "[display]\nkey_hints = false\n",
        Config {
            display: Display {
                key_hints: false,
                ..Display::default()
            },
            ..Config::default()
        }
    );
    writes!(
        display_key_hints_is_written,
        Config {
            display: Display {
                key_hints: false,
                ..Display::default()
            },
            ..Config::default()
        },
        "key_hints = false"
    );

    reads!(
        display_clock_is_read,
        "[display]\nclock = false\n",
        Config {
            display: Display {
                clock: false,
                ..Display::default()
            },
            ..Config::default()
        }
    );
    writes!(
        display_clock_is_written,
        Config {
            display: Display {
                clock: false,
                ..Display::default()
            },
            ..Config::default()
        },
        "clock = false"
    );

    reads!(
        display_side_pane_is_read,
        "[display]\nside_pane = false\n",
        Config {
            display: Display {
                side_pane: false,
                ..Display::default()
            },
            ..Config::default()
        }
    );
    writes!(
        display_side_pane_is_written,
        Config {
            display: Display {
                side_pane: false,
                ..Display::default()
            },
            ..Config::default()
        },
        "side_pane = false"
    );

    reads!(
        display_default_tab_is_read,
        "[display]\ndefault_tab = \"lyrics\"\n",
        Config {
            display: Display {
                default_tab: Tab::Lyrics,
                ..Display::default()
            },
            ..Config::default()
        }
    );
    writes!(
        display_default_tab_is_written,
        Config {
            display: Display {
                default_tab: Tab::Lyrics,
                ..Display::default()
            },
            ..Config::default()
        },
        "default_tab = \"lyrics\""
    );

    reads!(
        display_border_is_read,
        "[display]\nborder = \"double\"\n",
        Config {
            display: Display {
                border: Border::Double,
                ..Display::default()
            },
            ..Config::default()
        }
    );
    writes!(
        display_border_is_written,
        Config {
            display: Display {
                border: Border::Double,
                ..Display::default()
            },
            ..Config::default()
        },
        "border = \"double\""
    );

    reads!(
        display_accent_is_read,
        "[display]\naccent = \"green\"\n",
        Config {
            display: Display {
                accent: Accent::Green,
                ..Display::default()
            },
            ..Config::default()
        }
    );
    writes!(
        display_accent_is_written,
        Config {
            display: Display {
                accent: Accent::Green,
                ..Display::default()
            },
            ..Config::default()
        },
        "accent = \"green\""
    );

    reads!(
        display_art_protocol_is_read,
        "[display]\nart_protocol = \"halfblocks\"\n",
        Config {
            display: Display {
                art_protocol: ArtProtocol::HalfBlocks,
                ..Display::default()
            },
            ..Config::default()
        }
    );
    writes!(
        display_art_protocol_is_written,
        Config {
            display: Display {
                art_protocol: ArtProtocol::HalfBlocks,
                ..Display::default()
            },
            ..Config::default()
        },
        "art_protocol = \"halfblocks\""
    );

    // ------------------------------------------------------------- [visualizer]

    reads!(
        visualizer_style_is_read,
        "[visualizer]\nstyle = \"circular\"\n",
        Config {
            visualizer: Visualizer {
                style: VisualizerStyle::Circular,
                ..Visualizer::default()
            },
            ..Config::default()
        }
    );
    writes!(
        visualizer_style_is_written,
        Config {
            visualizer: Visualizer {
                style: VisualizerStyle::Circular,
                ..Visualizer::default()
            },
            ..Config::default()
        },
        "style = \"circular\""
    );

    reads!(
        visualizer_source_is_read,
        "[visualizer]\nsource = \"simulated\"\n",
        Config {
            visualizer: Visualizer {
                source: VisualizerSource::Simulated,
                ..Visualizer::default()
            },
            ..Config::default()
        }
    );
    writes!(
        visualizer_source_is_written,
        Config {
            visualizer: Visualizer {
                source: VisualizerSource::Simulated,
                ..Visualizer::default()
            },
            ..Config::default()
        },
        "source = \"simulated\""
    );

    // ------------------------------------------------------------------- [input]

    reads!(
        input_mouse_is_read,
        "[input]\nmouse = false\n",
        Config {
            input: Input {
                mouse: false,
                ..Input::default()
            },
            ..Config::default()
        }
    );
    writes!(
        input_mouse_is_written,
        Config {
            input: Input {
                mouse: false,
                ..Input::default()
            },
            ..Config::default()
        },
        "mouse = false"
    );

    reads!(
        input_volume_step_is_read,
        "[input]\nvolume_step = 25\n",
        Config {
            input: Input {
                volume_step: 25,
                ..Input::default()
            },
            ..Config::default()
        }
    );
    writes!(
        input_volume_step_is_written,
        Config {
            input: Input {
                volume_step: 25,
                ..Input::default()
            },
            ..Config::default()
        },
        "volume_step = 25"
    );

    reads!(
        input_seek_step_is_read,
        "[input]\nseek_step = 15\n",
        Config {
            input: Input {
                seek_step: 15.0,
                ..Input::default()
            },
            ..Config::default()
        }
    );
    writes!(
        input_seek_step_is_written,
        Config {
            input: Input {
                seek_step: 15.0,
                ..Input::default()
            },
            ..Config::default()
        },
        "seek_step = 15"
    );

    // ------------------------------------------------------------------ [volume]

    reads!(
        volume_control_is_read,
        "[volume]\ncontrol = \"system\"\n",
        Config {
            volume: Volume {
                control: VolumeControl::System
            },
            ..Config::default()
        }
    );
    writes!(
        volume_control_is_written,
        Config {
            volume: Volume {
                control: VolumeControl::System
            },
            ..Config::default()
        },
        "control = \"system\""
    );

    // ----------------------------------------------------------- [notifications]

    reads!(
        notifications_song_change_is_read,
        "[notifications]\nsong_change = true\n",
        Config {
            notifications: Notifications { song_change: true },
            ..Config::default()
        }
    );
    writes!(
        notifications_song_change_is_written,
        Config {
            notifications: Notifications { song_change: true },
            ..Config::default()
        },
        "song_change = true"
    );

    // ------------------------------------------------------------------ [lyrics]

    reads!(
        lyrics_enabled_is_read,
        "[lyrics]\nenabled = false\n",
        Config {
            lyrics: Lyrics { enabled: false },
            ..Config::default()
        }
    );
    writes!(
        lyrics_enabled_is_written,
        Config {
            lyrics: Lyrics { enabled: false },
            ..Config::default()
        },
        "enabled = false"
    );

    // ----------------------------------------------------------------- [spotify]

    reads!(
        spotify_client_id_is_read,
        "[spotify]\nclient_id = \"4c2b19f7a1\"\n",
        Config {
            spotify: Spotify {
                client_id: "4c2b19f7a1".into()
            },
            ..Config::default()
        }
    );
    writes!(
        spotify_client_id_is_written,
        Config {
            spotify: Spotify {
                client_id: "4c2b19f7a1".into()
            },
            ..Config::default()
        },
        "client_id = \"4c2b19f7a1\""
    );

    // ---------------------------------------------------------- the file itself

    /// SPEC §8's file read back is every default, which is the other half of
    /// "the defaults are the ones in the spec".
    #[test]
    fn the_specs_file_is_all_defaults() {
        assert_eq!(loaded(SPEC), Config::default());
    }

    /// The file trak writes is SPEC §8's file: same sections, same names, same
    /// order, same comments, same column. TODO 5.2's screen and the docs quote
    /// that block, so a key written under a different name is a bug here and in
    /// every one of them.
    #[test]
    fn the_file_that_is_written_is_the_specs_file() {
        let toml = Config::default().to_toml();
        assert!(toml.contains(SPEC), "not SPEC's shape:\n{toml}");
    }

    /// A value longer than the comment column still gets a space before its
    /// comment. A 32-character Client ID does, and without this it was written
    /// as `client_id = "..."# empty = Version B` (2026-10-02).
    #[test]
    fn a_value_longer_than_the_comment_column_is_still_spaced() {
        let toml = Config {
            spotify: Spotify {
                client_id: "e7d2504e3b0f49faab742cb8315b83fc".into(),
            },
            ..Config::default()
        }
        .to_toml();
        assert!(
            toml.contains("client_id = \"e7d2504e3b0f49faab742cb8315b83fc\" #"),
            "no space before the comment:\n{toml}"
        );
    }

    /// Every key at a non-default value, through the filesystem, and back.
    #[test]
    fn every_key_survives_a_save_and_a_load() {
        let (_home, paths) = sandbox("roundtrip");
        let config = everything();
        config.save_to(&paths.file()).expect("save");
        let loaded_file = Config::load_from(&paths.file()).expect("load");
        assert_eq!(loaded_file.config, config, "the whole config");
        assert_eq!(
            loaded_file.recovered,
            Recovered::None,
            "and nothing went wrong"
        );
        assert_eq!(loaded_file.notice(), None, "a good load says nothing");
        // And the settings the TUI reads are the settings that were set.
        assert_eq!(loaded_file.config.settings(), config.settings());
    }

    /// Saving twice writes the same bytes: a screen that saves on leaving is not
    /// churning the file, and a diff of it is readable.
    #[test]
    fn saving_twice_writes_the_same_bytes() {
        let (_home, paths) = sandbox("stable");
        let config = everything();
        config.save_to(&paths.file()).expect("first");
        let first = fs::read_to_string(paths.file()).expect("read");
        config.save_to(&paths.file()).expect("second");
        assert_eq!(first, fs::read_to_string(paths.file()).expect("read"));
        assert_eq!(loaded(&first), config);
    }

    /// The defaults in this module are the defaults the TUI already had, in every
    /// field. If this fails then `Settings` gained or changed a field and the
    /// mapping in `apply` is the thing that is out of date.
    #[test]
    fn the_defaults_are_the_settings_defaults() {
        assert_eq!(Config::default().settings(), Settings::default());
        assert_eq!(
            Config::default().with_settings(&Settings::default()),
            Config::default()
        );
    }

    /// The hop the settings screen (TODO 5.2) saves through. Every key both sides
    /// have survives it, the Client ID included -- which is the reason the hop
    /// starts from the config rather than from a default: a screen that cannot
    /// edit the Client ID must not delete it on save.
    #[test]
    fn a_config_survives_a_round_trip_through_settings() {
        let mut config = everything();
        for border in Border::ALL {
            config.display.border = border;
            assert_eq!(
                config.with_settings(&config.settings()),
                config,
                "{border:?}"
            );
        }
        assert_eq!(
            config.with_settings(&config.settings()).spotify,
            config.spotify,
            "the Client ID is untouched"
        );
        // And `apply` moves nothing that is not in both.
        let mut settings = Settings::default();
        config.apply(&mut settings);
        assert_eq!(settings, config.settings());
    }

    // --------------------------------------------------------------- the reader

    /// No file at all is every default with nothing to say about it. That is the
    /// first-run path (TODO 5.4) and it has to be silent, because most people
    /// never have this file.
    #[test]
    fn a_missing_file_is_every_default_and_not_an_error() {
        let (_home, paths) = sandbox("missing");
        assert!(!paths.file().exists());
        let loaded_file = Config::load_from(&paths.file()).expect("not a failure");
        assert_eq!(loaded_file.config, Config::default());
        assert_eq!(loaded_file.path, paths.file());
        assert_eq!(loaded_file.recovered, Recovered::None);
        assert_eq!(loaded_file.notice(), None);
    }

    /// An empty file is the same thing: trak writes the whole file, and a person
    /// may empty it to get back to the defaults.
    #[test]
    fn an_empty_file_is_every_default() {
        let (_home, paths) = sandbox("empty");
        for body in ["", "\n\n", "# nothing but a comment\n", "[display]\n"] {
            let loaded_file = load_file(paths.dir(), body);
            assert_eq!(loaded_file.config, Config::default(), "{body:?}");
            assert_eq!(loaded_file.recovered, Recovered::None, "{body:?}");
        }
    }

    /// The keys that are present win and the ones that are not are defaults. This
    /// is the case nobody tests and everybody gets wrong: a config *replaced* by
    /// what the file said rather than merged over the defaults loses every key the
    /// person did not touch.
    #[test]
    fn a_partly_set_config_keeps_the_rest_of_its_defaults() {
        let config = loaded(concat!(
            "[display]\nmode = \"visualizer\"\n\n",
            "[input]\nseek_step = 15\n"
        ));
        assert_eq!(
            config,
            Config {
                display: Display {
                    mode: DisplayMode::Visualizer,
                    ..Display::default()
                },
                input: Input {
                    seek_step: 15.0,
                    ..Input::default()
                },
                ..Config::default()
            }
        );
    }

    /// Unknown keys and unknown tables are ignored and the keys around them are
    /// still read. This is the forward-compatibility rule, and it is the whole
    /// reason a file written by a newer trak is not a corrupt one: a key with a
    /// date in it, a list, a table of its own, none of which this build knows.
    #[test]
    fn unknown_keys_and_tables_are_ignored() {
        let config = loaded(concat!(
            "# written by a trak from 2026\n",
            "[future]\n",
            "schema = 3\n",
            "when = 2026-01-02T03:04:05Z\n",
            "tabs = [\"a\", \"b\"]\n",
            "note = { nested = true }\n",
            "\n",
            "[display]\n",
            "art = false\n",
            "written_by = \"somebody\"\n",
            "volume = true\n",
            "\n",
            "[another_table_they_added]\n",
            "loudness = 0.5\n",
        ));
        assert_eq!(
            config,
            Config {
                display: Display {
                    art: false,
                    ..Display::default()
                },
                ..Config::default()
            },
            "the known keys around them still count"
        );
    }

    /// A key written before any table header is in the root table, which is not one
    /// trak reads. Reading it as if it were in the first header would apply a stray
    /// setting to whatever section happened to come next.
    #[test]
    fn keys_before_any_header_are_in_the_root_table_and_are_ignored() {
        let config = loaded("art = false\nmode = \"visualizer\"\n[display]\nclock = false\n");
        assert_eq!(
            config,
            Config {
                display: Display {
                    clock: false,
                    ..Display::default()
                },
                ..Config::default()
            }
        );
    }

    /// A value of the wrong kind leaves that one key at its default and leaves the
    /// file a file. Throwing forty good keys away over `volume = "loud"` is the
    /// failure this exists to prevent, and it is also what a corrupt file does.
    #[test]
    fn a_wrong_value_type_only_costs_that_key() {
        let config = loaded(concat!(
            "[display]\n",
            "volume = \"loud\"\n",
            "mode = 3\n",
            "accent = [\"green\"]\n",
            "art_protocol = { a = 1 }\n",
            "border = \"wobbly\"\n",
            "default_tab = \"nowhere\"\n",
            "art = false\n",
            "[input]\n",
            "volume_step = \"ten\"\n",
            "volume_step = 300\n",
            "seek_step = true\n",
            "mouse = 1\n",
            "[spotify]\n",
            "client_id = 42\n",
            "[volume]\n",
            "control = \"everything\"\n",
        ));
        assert_eq!(
            config,
            Config {
                display: Display {
                    art: false,
                    ..Display::default()
                },
                ..Config::default()
            }
        );
    }

    /// The whitespace a hand-edited file has: a byte-order mark from TextEdit,
    /// CRLF line endings from Windows, tabs, blank lines, and comments in every
    /// place one can go.
    #[test]
    fn comments_quotes_blank_lines_tabs_and_crlf_are_all_read() {
        let config = loaded(concat!(
            "\u{feff}",
            "# trak\r\n",
            "\r\n",
            "\t[ \"display\" ]\t\r\n",
            "\t\"art\"   =   false\t# off, for now\r\n",
            "\r\n",
            "side_pane= false # and this one too\r\n",
            "[ input ]\r\n",
            "mouse = false\r\n",
        ));
        assert_eq!(
            config,
            Config {
                display: Display {
                    art: false,
                    side_pane: false,
                    ..Display::default()
                },
                input: Input {
                    mouse: false,
                    ..Input::default()
                },
                ..Config::default()
            }
        );
    }

    /// A key written twice takes the first, which is the line somebody is looking
    /// at when they append to a section.
    #[test]
    fn a_repeated_key_takes_the_first() {
        assert!(!loaded("[display]\nart = false\nart = true\n").display.art);
    }

    /// A table written in two blocks is one table, not a second one: trak's own
    /// writer never does it, but a person adding two lines to a long file will.
    #[test]
    fn a_repeated_table_is_one_table() {
        let config = loaded(concat!(
            "[display]\nart = false\n",
            "[input]\nmouse = false\n",
            "[display]\nmode = \"visualizer\"\n"
        ));
        assert_eq!(
            config,
            Config {
                display: Display {
                    art: false,
                    mode: DisplayMode::Visualizer,
                    ..Display::default()
                },
                input: Input {
                    mouse: false,
                    ..Input::default()
                },
                ..Config::default()
            }
        );
    }

    /// Numbers in the shapes people type them. `5` and `5.0` are the same value
    /// and both are read, because a person editing a seek step is not making a
    /// mistake by writing one of them.
    #[test]
    fn numbers_are_read_in_the_shapes_people_write_them() {
        for (text, seek) in [
            ("5", 5.0),
            ("5.0", 5.0),
            ("2.5", 2.5),
            ("-3", -3.0),
            ("1e1", 10.0),
        ] {
            assert_eq!(
                loaded(&format!("[input]\nseek_step = {text}\n"))
                    .input
                    .seek_step,
                seek,
                "{text}"
            );
        }
        for (text, step) in [("+5", 5), ("5_0", 50), ("-5", -5), ("0", 0)] {
            assert_eq!(
                loaded(&format!("[input]\nvolume_step = {text}\n"))
                    .input
                    .volume_step,
                step,
                "{text}"
            );
        }
        // A whole number is written without a point, because that is how a person
        // writes it and how SPEC §8 writes it.
        assert!(Config::default().to_toml().contains("seek_step = 5\n"));
        // And a fraction keeps its point, because dropping one changes the value.
        let fractional = Config {
            input: Input {
                seek_step: 2.5,
                ..Input::default()
            },
            ..Config::default()
        };
        assert!(fractional.to_toml().contains("seek_step = 2.5\n"));
    }

    /// The escapes TOML has that this reader honours.
    #[test]
    fn escapes_are_decoded() {
        assert_eq!(
            loaded("[spotify]\nclient_id = \"a\\u0062\\tc\\\\d\\\"e\"\n")
                .spotify
                .client_id,
            "ab\tc\\d\"e"
        );
        // A literal string has no escapes at all, which is what somebody means
        // when they write a path in one.
        assert_eq!(
            loaded("[spotify]\nclient_id = 'C:\\keys\\x'\n")
                .spotify
                .client_id,
            "C:\\keys\\x"
        );
    }

    /// A string a person can type round-trips, whatever is in it. Without the
    /// escaping on the way out this would write a file that reads as corrupt,
    /// which is the worst outcome this module has.
    #[test]
    fn a_string_with_quotes_and_newlines_round_trips() {
        let config = Config {
            spotify: Spotify {
                client_id: "a\"b\\c\nd\te\u{7}f\u{1b}g".into(),
            },
            ..Config::default()
        };
        let toml = config.to_toml();
        assert_eq!(loaded(&toml), config, "{toml}");
    }

    // ------------------------------------------------------------ what is corrupt

    /// The two things a person can get wrong that change the meaning of the rest of
    /// the file: a string that never closes and a key with no `=`. Both are
    /// corrupt, and both say which line.
    #[test]
    fn an_unterminated_string_or_a_missing_equals_is_corrupt() {
        let error = parse(b"[display]\nart = true\nmode = \"art\n").unwrap_err();
        assert!(
            matches!(error, ParseError::Line { line: 3, .. }),
            "{error:?}"
        );
        assert!(error.notice().contains("line 3"), "{}", error.notice());
    }

    /// Everything else this reader refuses, and the one thing it refuses not to:
    /// `art = tru` is a legal file that says nothing about `art`, which is not the
    /// same as a file it cannot read.
    #[test]
    fn a_broken_line_is_corrupt() {
        for body in [
            "mode = \"art\n",
            "[display\nart = true\n",
            "art true\n",
            "art =\n",
            "art = \"a\" \"b\"\n",
            "art = true extra\n",
            "= 5\n",
            "[]\n",
            "\"display\n",
            "[display]\n[display]\nart\n",
            "art = \"a\\q\"\n",
            "art = \"a\\u00\"\n",
            "art = \"a\\uD83C\"\n",
            "art = \"a\nb\"\n",
            "tabs = [1, 2\n",
            "tabs = [[[[[[[[[1]]]]]]]]]\n",
            "note = { a = 1\n",
            "art = true # a comment with a \" in it is fine, this line is not\n[display]\nart\n",
        ] {
            assert!(
                matches!(parse(body.as_bytes()), Err(ParseError::Line { .. })),
                "{body:?} should be corrupt"
            );
        }
        // A bare word that is not a boolean is a value trak does not read, not a
        // broken file: this is what keeps a truncated *value* from resetting the
        // other forty keys.
        assert!(
            loaded("[display]\nart = tru\n").display.art,
            "still the default"
        );
    }

    /// A file that is not text, and one that is not a config, are both refused
    /// rather than half-read.
    #[test]
    fn a_file_that_is_not_text_or_is_too_big_is_corrupt() {
        assert_eq!(
            parse(b"[display]\nart = \xff\xfe\n").unwrap_err(),
            ParseError::NotText
        );
        let mut body = String::from("[display]\n");
        body.push_str(&"# padding\n".repeat(MAX_BYTES as usize));
        assert!(matches!(
            parse(body.as_bytes()).unwrap_err(),
            ParseError::TooLarge { .. }
        ));
    }

    /// The reader never panics on a truncated file, at any cut. trak writes the
    /// config with a rename so it should never see half of one, but "should never"
    /// is not a reason to have an unchecked index.
    ///
    /// Not every cut is an error the way a truncated JSON document is: TOML is
    /// line-based, so a cut in the middle of a word leaves a bare token, which is a
    /// legal file that says nothing. The count is asserted instead, so a change to
    /// which cuts are refused is a visible one.
    #[test]
    fn a_truncated_config_never_panics() {
        let mut refused = 0;
        for cut in 0..SPEC.len() {
            match parse(&SPEC.as_bytes()[..cut]) {
                Ok(_) => {}
                Err(ParseError::Line { .. }) => refused += 1,
                Err(other) => panic!("cut at {cut} gave {other:?}"),
            }
        }
        assert!(
            refused > 100 && refused < SPEC.len() / 2,
            "{refused} of {} cuts were refused",
            SPEC.len()
        );
        // Every cut inside a quoted value is refused, because a string that never
        // closes is exactly what a half-written file looks like.
        // The opening quote of `mode = "art"`, and the four cuts before the string
        // closes.
        let mode = SPEC.find("mode = ").expect("the spec has a mode") + 8;
        for cut in mode..mode + 4 {
            assert!(
                parse(&SPEC.as_bytes()[..cut]).is_err(),
                "cut at {cut} is inside a string"
            );
        }
    }

    // ----------------------------------------------------------------- the disk

    /// A file that cannot be parsed is kept, renamed, and the defaults are used.
    /// The bytes in the backup are the bytes that were there: not deleting it is
    /// the entire point, because somebody has to be able to go and fix it.
    #[test]
    fn a_corrupt_file_is_moved_aside_and_the_defaults_are_used() {
        let (_home, paths) = sandbox("corrupt");
        let body = "[display]\nart = \"unterminated\n";
        fs::create_dir_all(paths.dir()).expect("config dir");
        fs::write(paths.file(), body).expect("write");
        let loaded_file = Config::load_from(&paths.file()).expect("load");
        assert_eq!(loaded_file.config, Config::default());
        assert_eq!(loaded_file.path, paths.file());
        assert!(
            !paths.file().exists(),
            "the broken file is not left in place"
        );
        assert!(
            matches!(&loaded_file.recovered, Recovered::Moved { backup, .. } if backup == &paths.backup()),
            "{:?}",
            loaded_file.recovered
        );
        assert_eq!(fs::read(paths.backup()).expect("backup"), body.as_bytes());
        let notice = loaded_file.notice().expect("something to say");
        assert!(notice.contains("config.toml.bak"), "{notice:?}");
    }

    /// A second broken file goes to the same place and replaces the first backup:
    /// there is one config, and a rename is a rename.
    #[test]
    fn a_second_corrupt_file_replaces_the_first_backup() {
        let (_home, paths) = sandbox("corrupt-twice");
        fs::create_dir_all(paths.dir()).expect("config dir");
        for body in ["art = \n", "art = \"a\n"] {
            fs::write(paths.file(), body).expect("write");
            assert_eq!(
                Config::load_from(&paths.file()).expect("load").config,
                Config::default()
            );
        }
        assert_eq!(
            fs::read_to_string(paths.backup()).expect("backup"),
            "art = \"a\n"
        );
    }

    /// A broken file that cannot be moved aside: the defaults are still what is
    /// loaded, and the caller is told the file is still there. A directory in the
    /// way of the backup is how a test forces this without permissions, which
    /// matters because it is also the case that has to work as root.
    #[test]
    fn a_corrupt_file_that_cannot_be_moved_aside_is_still_readable() {
        let (_home, paths) = sandbox("kept");
        fs::create_dir_all(paths.backup()).expect("dir in the way");
        let body = "art = \n";
        fs::create_dir_all(paths.dir()).expect("config dir");
        fs::write(paths.file(), body).expect("write");
        let loaded_file = Config::load_from(&paths.file()).expect("load");
        assert_eq!(loaded_file.config, Config::default());
        assert!(matches!(loaded_file.recovered, Recovered::Kept { .. }));
        assert_eq!(
            fs::read_to_string(paths.file()).expect("still there"),
            body,
            "it is not deleted either"
        );
        let notice = loaded_file.notice().expect("something to say");
        assert!(notice.contains("could not be set aside"), "{notice:?}");
    }

    /// A file that is not a config at all -- a log, a swap file, a symlink to one
    /// -- is moved aside like any other file that cannot be read, rather than read
    /// into memory first.
    #[test]
    fn a_huge_file_is_corrupt_and_never_read_whole() {
        let (_home, paths) = sandbox("huge");
        fs::create_dir_all(paths.dir()).expect("config dir");
        let body = format!("[display]\n{}", "# padding\n".repeat(MAX_BYTES as usize));
        assert!(body.len() as u64 > MAX_BYTES);
        fs::write(paths.file(), &body).expect("write");
        let loaded_file = Config::load_from(&paths.file()).expect("load");
        assert_eq!(loaded_file.config, Config::default());
        assert!(matches!(loaded_file.recovered, Recovered::Moved { .. }));
        // The bytes are kept whole: the reader bounded itself, it did not truncate.
        assert_eq!(fs::read(paths.backup()).expect("backup"), body.as_bytes());
    }

    /// A directory where the file should be is a disk failure, not a corrupt file:
    /// nothing is moved aside and the caller gets one line.
    #[test]
    fn a_directory_where_the_file_should_be_is_an_error() {
        let (_home, paths) = sandbox("isdir");
        fs::create_dir_all(paths.file()).expect("dir");
        assert!(matches!(
            Config::load_from(&paths.file()),
            Err(ConfigError::Read { .. })
        ));
    }

    /// A first save makes the directory. This is the first-run path (TODO 5.4)
    /// writing a file where there was never one.
    #[test]
    fn a_first_save_creates_its_directory() {
        let (_home, paths) = sandbox("mkdir");
        assert!(!paths.dir().exists());
        Config::default().save_to(&paths.file()).expect("save");
        assert_eq!(
            Config::load_from(&paths.file()).expect("load").config,
            Config::default()
        );
    }

    /// The permissions of a file a person will edit: 0600. The Client ID is not a
    /// secret, but the file is the owner's settings and this costs nothing.
    #[test]
    fn a_saved_config_is_owner_only() {
        let (_home, paths) = sandbox("mode");
        Config::default().save_to(&paths.file()).expect("save");
        assert_eq!(mode_of(&paths.file()), 0o600);
    }

    /// A save publishes with a rename, so nothing half-written is left beside the
    /// file, and it fixes the mode of a file somebody loosened.
    #[test]
    fn a_save_replaces_a_loose_file_and_leaves_nothing_behind() {
        let (_home, paths) = sandbox("replace");
        fs::create_dir_all(paths.dir()).expect("config dir");
        fs::write(paths.file(), "junk\n").expect("write");
        fs::set_permissions(paths.file(), fs::Permissions::from_mode(0o644)).expect("loosen");
        Config::default().save_to(&paths.file()).expect("save");
        assert_eq!(
            mode_of(&paths.file()),
            0o600,
            "the mode is set, not inherited"
        );
        let names: Vec<String> = fs::read_dir(paths.dir())
            .expect("read dir")
            .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![FILE_NAME.to_string()], "no temp file survives");
        assert_eq!(
            loaded(&fs::read_to_string(paths.file()).expect("read")),
            Config::default()
        );
    }

    // ------------------------------------------------------------------- paths

    /// `XDG_CONFIG_HOME` wins when it is set, `HOME/.config` is the fallback
    /// (SPEC §8), and neither is read by the loader -- both come in through
    /// `Paths`, which is what makes these tests safe to run.
    #[test]
    fn xdg_config_home_wins_and_home_is_the_fallback() {
        let home = PathBuf::from("/home/someone");
        let xdg = PathBuf::from("/xdg");
        assert_eq!(
            Paths::from_vars(Some(xdg.clone()), Some(home.clone())).file(),
            xdg.join("trak").join(FILE_NAME)
        );
        assert_eq!(
            Paths::from_vars(Some(xdg.clone()), None).file(),
            xdg.join("trak").join(FILE_NAME)
        );
        assert_eq!(
            Paths::from_vars(None, Some(home.clone())).file(),
            home.join(".config").join("trak").join(FILE_NAME)
        );
        // And neither is not a panic: the path is relative, and a load from it is
        // an ordinary "could not read".
        let neither = Paths::from_vars(None, None);
        assert_eq!(
            neither.file(),
            PathBuf::from(".config/trak").join(FILE_NAME)
        );
        assert!(neither.file().ends_with("trak/config.toml"));
    }

    /// A save of a bare file name lands in the current directory, because the empty
    /// parent is not a directory that can be created.
    #[test]
    fn a_bare_file_name_saves_to_the_current_directory() {
        assert_eq!(dir_of(Path::new("config.toml")), Path::new("."));
        assert_eq!(
            dir_of(Path::new("/tmp/trak/config.toml")),
            Path::new("/tmp/trak")
        );
        assert_eq!(
            backup_for(Path::new("config.toml")),
            PathBuf::from("config.toml.bak")
        );
        assert_eq!(
            backup_for(Path::new("/tmp/trak/config.toml")),
            PathBuf::from("/tmp/trak/config.toml.bak")
        );
        assert_eq!(
            temp_path(Path::new("/tmp/trak/config.toml")),
            PathBuf::from("/tmp/trak/.config.toml.tmp")
        );
        // And a path that names no file at all still names this module's.
        assert_eq!(name_of(Path::new("/")), FILE_NAME);
    }

    /// The one place the real environment is read.
    #[test]
    fn the_paths_from_the_environment_are_a_config_path() {
        let paths = Paths::from_env();
        assert!(
            paths.file().ends_with("trak/config.toml"),
            "{:?}",
            paths.file()
        );
        assert_eq!(
            paths.backup(),
            paths.dir().join("config.toml.bak"),
            "and the backup is derived, not stored"
        );
    }

    // -------------------------------------------------------------- the new types

    /// Every word trak writes for these three, it can read; a word it does not
    /// know is `None` rather than a guess. Version A's tab names are the case
    /// that matters: this build has no `search` tab, so it must not invent one.
    #[test]
    fn the_new_enums_parse_their_own_words() {
        for protocol in ArtProtocol::ALL {
            assert_eq!(ArtProtocol::parse(protocol.label()), Some(protocol));
        }
        for source in VisualizerSource::ALL {
            assert_eq!(VisualizerSource::parse(source.label()), Some(source));
        }
        for tab in Tab::ALL {
            assert_eq!(tab_from_config(tab_name(tab)), Some(tab));
        }
        assert_eq!(
            ArtProtocol::parse(" halfblocks "),
            Some(ArtProtocol::HalfBlocks),
            "surrounding space is not part of the word"
        );
        // Case is not either, and that is `theme.rs`'s and `app.rs`'s rule too: a
        // word that is not one of ours is a default rather than a guess.
        assert_eq!(ArtProtocol::parse("HALFBLOCKS"), None);
        assert_eq!(ArtProtocol::parse("kitty-gfx"), None);
        assert_eq!(VisualizerSource::parse("real"), None);
        assert_eq!(
            tab_from_config("nowhere"),
            None,
            "a word that is not a tab at all"
        );
        // The Version A names *are* tabs now (TODO 7.13), and a config written on
        // a build that had them must still open on one that does not -- so the
        // unknown-word rule is the only rule, with no "unknown to this build"
        // exception that would silently reset somebody's default tab.
        for tab in crate::tui::app::Tab::VERSION_A {
            assert_eq!(
                tab_from_config(tab_name(tab)),
                Some(tab),
                "{}",
                tab_name(tab)
            );
        }
        // And cycling wraps, for the settings screen's `left`/`right`.
        assert_eq!(ArtProtocol::Auto.next(), ArtProtocol::Kitty);
        assert_eq!(ArtProtocol::HalfBlocks.next(), ArtProtocol::Auto);
        assert_eq!(VisualizerSource::Auto.next(), VisualizerSource::Simulated);
        assert_eq!(VisualizerSource::Simulated.next(), VisualizerSource::Auto);
    }

    /// Errors reach a toast, so every message is one line and says which file.
    #[test]
    fn error_messages_are_one_line() {
        let (_home, paths) = sandbox("notice");
        fs::create_dir_all(paths.file()).expect("dir");
        for error in [
            ConfigError::Read { path: paths.file() },
            ConfigError::Mkdir {
                path: paths.dir().to_path_buf(),
            },
            ConfigError::Write { path: paths.file() },
        ] {
            assert_eq!(error.notice(), error.to_string());
            assert!(error.notice().starts_with("trak: "), "{}", error.notice());
            assert!(!error.notice().contains('\n'), "{}", error.notice());
        }
        for error in [
            ParseError::NotText,
            ParseError::TooLarge { bytes: 9 },
            ParseError::Line {
                line: 2,
                reason: "a key needs an `=`",
            },
        ] {
            assert_eq!(error.notice(), error.to_string());
            assert!(!error.notice().contains('\n'), "{}", error.notice());
        }
    }

    /// The mode bits of a file, so the tests do not each spell this out.
    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }
}
