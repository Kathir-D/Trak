//! trak — an interactive terminal UI for the Spotify desktop app on macOS.
//!
//! A thin shell: parse argv, then hand off to the library. Everything that could
//! be tested lives there, so this file stays small enough to read at a glance.

use std::io::IsTerminal as _;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use trak::cli::{EXIT_FAIL, EXIT_OK, EXIT_USAGE, PlayGroup, Style};
use trak::player::AppleScriptPlayer;
use trak::player::Player;
use trak::web::api::Library;
use trak::web::token::Store as _;

/// Every shpotify-compatible command (SPEC §9).
#[derive(Parser)]
#[command(
    name = "trak",
    version,
    about = "A fast, interactive terminal UI for the Spotify desktop app on macOS",
    disable_help_subcommand = true
)]
struct Cli {
    /// No colour, no box drawing. Implied when stdout is not a terminal.
    #[arg(long, global = true)]
    plain: bool,

    /// Machine-readable output (status only).
    #[arg(long, global = true)]
    json: bool,

    /// Talk to an in-memory player instead of Spotify. Hidden, and only here so
    /// the CLI can be tested on a machine with no Spotify and in CI
    /// (ARCHITECTURE, "Testing strategy").
    #[arg(long, global = true, hide = true)]
    fake: bool,

    /// Talk to an in-memory library instead of the Web API. Hidden for the same
    /// reason as `--fake`. `--fake` on its own still means "no library" — the
    /// state of a machine that has never connected — so the setup path is
    /// testable too, not just the happy one.
    #[arg(long, global = true, hide = true)]
    fake_library: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Show what's playing.
    Status {
        /// Limit the output to one field.
        #[arg(value_enum)]
        field: Option<Field>,
    },
    /// Play, or resume if already playing.
    Play {
        /// A song to search for, or a Spotify URI to play, or nothing to resume.
        /// Several words are one search phrase, as they were for shpotify.
        #[arg(value_name = "NAME|URI", num_args = 0..)]
        words: Vec<String>,
        #[command(subcommand)]
        what: Option<PlayWhat>,
    },
    /// Pause playback. If already paused, resume — as shpotify does.
    Pause,
    /// Stop playback. Spotify has no `stop` command, so this pauses if playing
    /// and otherwise reports that it is already stopped (docs/APPLESCRIPT.md §6).
    Stop,
    /// Quit the Spotify app.
    Quit,
    /// Skip to the next track.
    Next,
    /// Go to the previous track.
    Prev,
    /// Restart the current track.
    Replay,
    /// Seek to a position, in seconds.
    Pos { secs: f64 },
    /// Show, set, or step the volume.
    Vol {
        #[arg(value_name = "ARG")]
        arg: Option<VolArg>,
    },
    /// Toggle shuffle or repeat.
    Toggle {
        /// What to toggle.
        #[arg(value_enum)]
        what: Mode,
    },
    /// Open the settings screen and save it on the way out (SPEC §8, TODO 5.2).
    ///
    /// The same screen the TUI's `,` opens, on its own: useful for editing before
    /// Spotify is ever running, and for reading the settings on a machine where
    /// launching the dashboard is not what you want.
    Config,
    /// Print, and for url/uri also copy, the current track's link.
    Share {
        /// `url` for a web link, `uri` for a spotify: URI.
        #[arg(value_enum, default_value = "url")]
        what: ShareArg,
    },
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum Field {
    Artist,
    Album,
    Track,
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum VolArg {
    Up,
    Down,
    Show,
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum Mode {
    Shuffle,
    Repeat,
}

#[derive(clap::ValueEnum, Clone, Copy)]
enum ShareArg {
    Url,
    Uri,
}

/// The qualified `play` spellings (SPEC §9), shpotify's own surface: search one
/// group and play the best match, or spell a URI out the way shpotify did.
#[derive(Subcommand)]
enum PlayWhat {
    /// Search albums and play the best match.
    Album {
        /// The album to look for.
        name: String,
    },
    /// Search artists and play the best match.
    Artist {
        /// The artist to look for.
        name: String,
    },
    /// Search playlists and play the best match.
    List {
        /// The playlist to look for.
        name: String,
    },
    /// Play a Spotify URI — the same thing the bare positional does, spelt the
    /// way shpotify did it.
    Uri {
        /// The URI to play.
        uri: String,
    },
}

/// The volume step. SPEC §8 makes it a setting; hard-coded until 5.x.
const VOLUME_STEP: u8 = 10;

fn style(plain: bool) -> Style {
    // `NO_COLOR` (no-color.org) is a request for no colour from any program, and
    // the plain style is the one without it (TODO 11.6).
    let no_color = std::env::var("NO_COLOR").is_ok_and(|v| !v.is_empty());
    if plain || no_color || !std::io::stdout().is_terminal() {
        Style::Plain
    } else {
        Style::Tidy
    }
}

fn main() -> ExitCode {
    run(Cli::parse())
}

/// Both player backends behind one call site, so every command is written once.
fn run(args: Cli) -> ExitCode {
    // The settings screen is handled before the player exists: it has no business
    // talking to Spotify, and `trak config` has to work on a machine where
    // Spotify is not running or not even installed (COMPAT rule 2 -- the only
    // launch trak ever performs is the idle card's).
    if matches!(args.command, Some(Command::Config)) {
        return ExitCode::from(trak::tui::config_screen() as u8);
    }

    let mut player: AnyPlayer = if args.fake {
        AnyPlayer::Fake(Box::new(trak::player::FakePlayer::playing()))
    } else {
        AnyPlayer::Real(Box::<AppleScriptPlayer>::default())
    };

    let code = match args.command {
        // Bare `trak` opens the TUI (SPEC §2).
        None => trak::tui::run(),
        // Unreachable: `config` returned above, before the player was built.
        Some(Command::Config) => EXIT_OK,
        Some(Command::Status { field }) => status(&player, args.json, style(args.plain), field),
        // The library is built here and nowhere else, so only `play` ever pays
        // for the token file existing — or not.
        Some(Command::Play { words, what }) => {
            let library = play_library(args.fake_library);
            play(
                &mut player,
                library.as_deref(),
                words,
                what,
                style(args.plain),
            )
        }
        Some(Command::Pause) => trak::cli::run_action(&mut player, "toggle"),
        Some(Command::Stop) => stop(&mut player),
        Some(Command::Quit) => quit(&mut player),
        Some(Command::Next) => trak::cli::run_action(&mut player, "next"),
        Some(Command::Prev) => trak::cli::run_action(&mut player, "prev"),
        Some(Command::Replay) => replay(&mut player),
        Some(Command::Pos { secs }) => trak::cli::run_seek(&mut player, secs),
        Some(Command::Vol { arg }) => vol(&mut player, arg, args.json),
        Some(Command::Toggle { what }) => mode(&mut player, what),
        Some(Command::Share { what }) => share(&player, what),
    };

    ExitCode::from(code as u8)
}

fn fail(e: trak::player::PlayerError) -> i32 {
    let (msg, code) = trak::cli::report(e);
    eprintln!("{msg}");
    code
}

fn status(player: &AnyPlayer, json: bool, style: Style, field: Option<Field>) -> i32 {
    let state = match player.state() {
        Ok(s) => s,
        Err(e) => return fail(e),
    };

    if let Some(f) = field {
        let v = match f {
            Field::Artist => &state.track.artist,
            Field::Album => &state.track.album,
            Field::Track => &state.track.title,
        };
        // shpotify printed these bare, so they stay bare.
        println!("{v}");
        return EXIT_OK;
    }

    if json {
        println!("{}", trak::cli::render_json(&state));
    } else {
        print!("{}", trak::cli::render_status(&state, style));
    }
    EXIT_OK
}

/// The library a `trak play <name>` searches, or `None` when there is nothing
/// to search through.
///
/// `None` is the pre-setup state of every machine, not a failure. The real half
/// makes the same read the TUI's `web_client` does and, like it, does not
/// refresh: renewing a stale access token is the login flow's job (TODO 7.3),
/// so the honest outcome is a "log in again" from the search, never a silent
/// second try.
fn play_library(fake: bool) -> Option<Box<dyn Library>> {
    if fake {
        return Some(Box::new(trak::web::api::FakeLibrary::seeded()));
    }
    let store = trak::web::token::TokenFile::at(&trak::config::Paths::from_env());
    let loaded = store.load().ok()?;
    let now = std::time::SystemTime::now();
    // The refresh half is what a search is about to spend; if it is past its
    // six months there is nothing to search with, only a login to redo.
    if loaded.token.as_ref().is_some_and(|t| t.refresh_stale(now)) {
        return None;
    }
    let token = loaded.token.as_ref()?;
    Some(Box::new(trak::web::api::SpotifyLibrary::new(token)))
}

/// `trak play`, in every spelling SPEC §9 gives it.
///
/// A URI plays straight away through AppleScript, which works on the Free
/// tier. A *name* needs search, which needs a connected Web API; without one
/// this says how to get there and exits 2 rather than pretending to have played
/// something — the message shpotify's users met for the same reason.
fn play(
    player: &mut AnyPlayer,
    library: Option<&dyn Library>,
    words: Vec<String>,
    what: Option<PlayWhat>,
    style: Style,
) -> i32 {
    match what {
        Some(PlayWhat::Uri { uri }) => play_uri(player, &uri),
        Some(PlayWhat::Album { name }) => {
            search_and_play(player, library, PlayGroup::Album, &name, style)
        }
        Some(PlayWhat::Artist { name }) => {
            search_and_play(player, library, PlayGroup::Artist, &name, style)
        }
        Some(PlayWhat::List { name }) => {
            search_and_play(player, library, PlayGroup::List, &name, style)
        }
        None => {
            if words.is_empty() {
                return trak::cli::run_action(player, "play");
            }
            // Several words are one phrase, joined the way shpotify joined its
            // arguments, so a search never needs quoting to be one query.
            let target = words.join(" ");
            let t = target.trim();
            if t.starts_with("spotify:") || t.contains("://") {
                return play_uri(player, t);
            }
            search_and_play(player, library, PlayGroup::Song, t, style)
        }
    }
}

/// A URI plays straight away through AppleScript, which works on the Free
/// tier. A user-supplied string is validated here, at the one point it enters
/// trak, so every backend enforces the same rule and none can be bypassed.
fn play_uri(player: &AnyPlayer, target: &str) -> i32 {
    if let Err(e) = trak::player::check_playable_uri(target) {
        let (msg, code) = trak::cli::report(e);
        eprintln!("{msg}");
        return code;
    }
    match player.play_uri(target) {
        Ok(()) => EXIT_OK,
        Err(e) => fail(e),
    }
}

/// Search one group, pick the best match the way shpotify did, and play it.
///
/// The "Playing …" line is printed only after the play landed, so trak never
/// says it is playing something Spotify refused — the next `trak status` is
/// the confirmation, the same as for every other play spelling.
fn search_and_play(
    player: &AnyPlayer,
    library: Option<&dyn Library>,
    group: PlayGroup,
    query: &str,
    style: Style,
) -> i32 {
    let Some(library) = library else {
        eprintln!(
            "`trak {spelling} \"{query}\"` needs to search Spotify, which needs a Client ID.\n\n\
             One-time setup:\n\
               1. open https://developer.spotify.com/dashboard\n\
               2. create an app, and add the redirect URI  http://127.0.0.1\n\
               3. run `trak config` and paste the Client ID\n\n\
             Until then you can play a URI directly:\n\
               trak play spotify:track:6HacgXCExkzS552ILfJTXu",
            spelling = group.spelling(),
        );
        return EXIT_USAGE;
    };

    let results = match library.search(query) {
        Ok(results) => results,
        Err(e) => {
            eprintln!("{}", e.notice());
            return EXIT_FAIL;
        }
    };
    let Some(choice) = trak::cli::choose(group, query, &results, style) else {
        // shpotify's own sentence, so a shpotify user is told the same thing
        // by the same miss.
        eprintln!("No results when searching for \"{query}\"");
        return EXIT_FAIL;
    };
    match player.play_uri(&choice.uri) {
        Ok(()) => {
            println!("{}", choice.line);
            EXIT_OK
        }
        Err(e) => fail(e),
    }
}

/// `stop`. Spotify's dictionary has no `stop` command — `tell ... to stop`
/// silently no-ops — so this is shpotify's behaviour: pause if playing, otherwise
/// say so (docs/APPLESCRIPT.md §6).
fn stop(player: &mut AnyPlayer) -> i32 {
    match player.state() {
        Ok(s) if s.playback == trak::player::PlaybackState::Playing => {
            println!("Pausing Spotify.");
            match player.pause() {
                Ok(()) => EXIT_OK,
                Err(e) => fail(e),
            }
        }
        Ok(_) => {
            println!("Spotify is already stopped.");
            EXIT_OK
        }
        Err(e) => fail(e),
    }
}

/// `quit`. A write, so it only ever runs because the user asked (COMPAT rule 3).
fn quit(player: &mut AnyPlayer) -> i32 {
    if !player.is_real() {
        eprintln!("the fake player cannot quit Spotify");
        return EXIT_USAGE;
    }
    match player.command("quit") {
        Ok(()) => {
            println!("Quitting Spotify.");
            EXIT_OK
        }
        Err(e) => fail(e),
    }
}

fn replay(player: &mut AnyPlayer) -> i32 {
    match player.state() {
        Ok(s) => match player.seek(0.0) {
            Ok(()) => {
                // shpotify's `replay` restarts without changing play state, so a
                // paused track stays paused.
                if s.playback == trak::player::PlaybackState::Paused {
                    let _ = player.pause();
                }
                EXIT_OK
            }
            Err(e) => fail(e),
        },
        Err(e) => fail(e),
    }
}

fn vol(player: &mut AnyPlayer, arg: Option<VolArg>, json: bool) -> i32 {
    match arg {
        None | Some(VolArg::Show) => match player.state() {
            Ok(s) => {
                if json {
                    println!("{{\"volume\":{}}}", s.volume);
                } else {
                    println!("{}", s.volume);
                }
                EXIT_OK
            }
            Err(e) => fail(e),
        },
        Some(VolArg::Up) => trak::cli::run_volume(player, VOLUME_STEP as i16, json),
        Some(VolArg::Down) => trak::cli::run_volume(player, -(VOLUME_STEP as i16), json),
    }
}

fn mode(player: &mut AnyPlayer, what: Mode) -> i32 {
    let state = match player.state() {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let line = match what {
        Mode::Shuffle => format!("set shuffling to {}", !state.shuffling_enabled),
        // AppleScript cannot read back "repeat one" as distinct from "repeat all",
        // so trak cycles its own remembered mode and writes the matching boolean.
        Mode::Repeat => {
            let next = player.repeat_mode().next();
            player.set_repeat_mode(next);
            format!("set repeating to {}", next != trak::player::RepeatMode::Off)
        }
    };
    // The same guarded path every other write uses, so the "never launch
    // Spotify" rule lives in exactly one place.
    match player.command(&line) {
        Ok(()) => EXIT_OK,
        Err(e) => fail(e),
    }
}

fn share(player: &AnyPlayer, what: ShareArg) -> i32 {
    let state = match player.state() {
        Ok(s) => s,
        Err(e) => return fail(e),
    };
    let Some(uri) = &state.track.uri else {
        eprintln!("nothing shareable is playing (an advert has no track link)");
        return EXIT_FAIL;
    };
    let link = match what {
        ShareArg::Uri => uri.clone(),
        ShareArg::Url => {
            let id = uri.rsplit(':').next().unwrap_or_default();
            format!("https://open.spotify.com/track/{id}")
        }
    };
    println!("{link}");
    // Copying needs a pasteboard write; failures are not worth failing over.
    if trak::player::copy_to_clipboard(&link) {
        use std::io::Write;
        let _ = std::io::stdout().flush();
    }
    EXIT_OK
}

/// Either player, erased to the trait.
///
/// An enum rather than `Box<dyn Player>` because a boxed trait object would have
/// to promise `Send`, which the CLI has no need for and the TUI worker does.
enum AnyPlayer {
    // Boxed because the fake is a RefCell-heavy struct and the real one holds
    // nothing; without this the enum is as large as its biggest variant for no
    // reason, since exactly one is ever live.
    Real(Box<AppleScriptPlayer>),
    Fake(Box<trak::player::FakePlayer>),
}

impl AnyPlayer {
    fn is_real(&self) -> bool {
        matches!(self, Self::Real(_))
    }

    fn repeat_mode(&self) -> trak::player::RepeatMode {
        match self {
            Self::Real(p) => p.repeat_mode(),
            Self::Fake(_) => trak::player::RepeatMode::Off,
        }
    }

    fn set_repeat_mode(&mut self, m: trak::player::RepeatMode) {
        if let Self::Real(p) = self {
            p.set_repeat_mode(m);
        }
    }
}

/// Delegates to whichever backend is active, so no call site has to care.
impl Player for AnyPlayer {
    fn state(&self) -> Result<trak::player::PlayerState, trak::player::PlayerError> {
        match self {
            Self::Real(p) => p.state(),
            Self::Fake(p) => p.state(),
        }
    }
    fn play(&self) -> Result<(), trak::player::PlayerError> {
        match self {
            Self::Real(p) => p.play(),
            Self::Fake(p) => p.play(),
        }
    }
    fn pause(&self) -> Result<(), trak::player::PlayerError> {
        match self {
            Self::Real(p) => p.pause(),
            Self::Fake(p) => p.pause(),
        }
    }
    fn toggle(&self) -> Result<(), trak::player::PlayerError> {
        match self {
            Self::Real(p) => p.toggle(),
            Self::Fake(p) => p.toggle(),
        }
    }
    fn next(&self) -> Result<(), trak::player::PlayerError> {
        match self {
            Self::Real(p) => p.next(),
            Self::Fake(p) => p.next(),
        }
    }
    fn previous(&self) -> Result<(), trak::player::PlayerError> {
        match self {
            Self::Real(p) => p.previous(),
            Self::Fake(p) => p.previous(),
        }
    }
    fn seek(&mut self, secs: f64) -> Result<(), trak::player::PlayerError> {
        match self {
            Self::Real(p) => p.seek(secs),
            Self::Fake(p) => p.seek(secs),
        }
    }
    fn set_volume(&mut self, v: u8) -> Result<(), trak::player::PlayerError> {
        match self {
            Self::Real(p) => p.set_volume(v),
            Self::Fake(p) => p.set_volume(v),
        }
    }
    fn volume(&self) -> Result<u8, trak::player::PlayerError> {
        match self {
            Self::Real(p) => p.volume(),
            Self::Fake(p) => p.volume(),
        }
    }
    fn play_uri(&self, uri: &str) -> Result<(), trak::player::PlayerError> {
        match self {
            Self::Real(p) => p.play_uri(uri),
            Self::Fake(p) => p.play_uri(uri),
        }
    }
    fn command(&self, script: &str) -> Result<(), trak::player::PlayerError> {
        match self {
            Self::Real(p) => p.command(script),
            Self::Fake(p) => p.command(script),
        }
    }
}
