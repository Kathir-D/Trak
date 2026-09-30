//! trak — an interactive terminal UI for the Spotify desktop app on macOS.
//!
//! `trak` on its own opens the TUI. Every other subcommand is a one-shot
//! command, kept compatible with shpotify (SPEC §9).

mod cli;
mod player;
#[cfg(test)]
mod testutil;
mod tui;

use std::io::IsTerminal as _;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::cli::{EXIT_FAIL, EXIT_OK, EXIT_USAGE, Style};
use crate::player::AppleScriptPlayer;
use crate::player::Player;

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
        /// A URI to play, or nothing to resume.
        #[arg(value_name = "URI")]
        uri: Option<String>,
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

/// The volume step. SPEC §8 makes it a setting; hard-coded until 5.x.
const VOLUME_STEP: u8 = 10;

fn style(plain: bool) -> Style {
    if plain || !std::io::stdout().is_terminal() {
        Style::Plain
    } else {
        Style::Tidy
    }
}

fn main() -> ExitCode {
    run(Cli::parse())
}

/// Both player backends behind one call site, so every command is written once.
///
/// `AppleScriptPlayer` is osascript; `FakePlayer` is the same trait in memory.
/// The fake exists so the CLI can be tested with no Spotify and in CI
/// (ARCHITECTURE, "Testing strategy"), which is what the hidden `--fake` flag
/// selects.
fn run(args: Cli) -> ExitCode {
    let mut player: AnyPlayer = if args.fake {
        AnyPlayer::Fake(Box::new(crate::player::FakePlayer::playing()))
    } else {
        AnyPlayer::Real(Box::<AppleScriptPlayer>::default())
    };

    let code = match args.command {
        // Bare `trak` opens the TUI (SPEC §2).
        None => tui::run(),
        Some(Command::Status { field }) => status(&player, args.json, style(args.plain), field),
        Some(Command::Play { uri: Some(uri) }) => play(&player, &uri),
        Some(Command::Play { uri: None }) => cli::run_action(&mut player, "play"),
        Some(Command::Pause) => cli::run_action(&mut player, "toggle"),
        Some(Command::Stop) => stop(&mut player),
        Some(Command::Quit) => quit(&mut player),
        Some(Command::Next) => cli::run_action(&mut player, "next"),
        Some(Command::Prev) => cli::run_action(&mut player, "prev"),
        Some(Command::Replay) => replay(&mut player),
        Some(Command::Pos { secs }) => cli::run_seek(&mut player, secs),
        Some(Command::Vol { arg }) => vol(&mut player, arg, args.json),
        Some(Command::Toggle { what }) => mode(&mut player, what),
        Some(Command::Share { what }) => share(&player, what),
    };

    ExitCode::from(code as u8)
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
    Fake(Box<crate::player::FakePlayer>),
}

impl AnyPlayer {
    fn real(&self) -> Option<&AppleScriptPlayer> {
        match self {
            Self::Real(p) => Some(p),
            Self::Fake(_) => None,
        }
    }

    fn repeat_mode(&self) -> crate::player::RepeatMode {
        match self {
            Self::Real(p) => p.repeat_mode(),
            Self::Fake(_) => crate::player::RepeatMode::Off,
        }
    }

    fn set_repeat_mode(&mut self, m: crate::player::RepeatMode) {
        if let Self::Real(p) = self {
            p.set_repeat_mode(m);
        }
    }
}

/// Delegates to whichever backend is active, so no call site has to care.
///
/// Written with method syntax so the boxed variants deref automatically.
impl crate::player::Player for AnyPlayer {
    fn state(&self) -> Result<crate::player::PlayerState, crate::player::PlayerError> {
        match self {
            Self::Real(p) => p.state(),
            Self::Fake(p) => p.state(),
        }
    }
    fn play(&self) -> Result<(), crate::player::PlayerError> {
        match self {
            Self::Real(p) => p.play(),
            Self::Fake(p) => p.play(),
        }
    }
    fn pause(&self) -> Result<(), crate::player::PlayerError> {
        match self {
            Self::Real(p) => p.pause(),
            Self::Fake(p) => p.pause(),
        }
    }
    fn toggle(&self) -> Result<(), crate::player::PlayerError> {
        match self {
            Self::Real(p) => p.toggle(),
            Self::Fake(p) => p.toggle(),
        }
    }
    fn next(&self) -> Result<(), crate::player::PlayerError> {
        match self {
            Self::Real(p) => p.next(),
            Self::Fake(p) => p.next(),
        }
    }
    fn previous(&self) -> Result<(), crate::player::PlayerError> {
        match self {
            Self::Real(p) => p.previous(),
            Self::Fake(p) => p.previous(),
        }
    }
    fn seek(&mut self, secs: f64) -> Result<(), crate::player::PlayerError> {
        match self {
            Self::Real(p) => p.seek(secs),
            Self::Fake(p) => p.seek(secs),
        }
    }
    fn set_volume(&mut self, v: u8) -> Result<(), crate::player::PlayerError> {
        match self {
            Self::Real(p) => p.set_volume(v),
            Self::Fake(p) => p.set_volume(v),
        }
    }
    fn play_uri(&self, uri: &str) -> Result<(), crate::player::PlayerError> {
        match self {
            Self::Real(p) => p.play_uri(uri),
            Self::Fake(p) => p.play_uri(uri),
        }
    }
    fn command(&self, script: &str) -> Result<(), crate::player::PlayerError> {
        match self {
            Self::Real(p) => p.command(script),
            Self::Fake(p) => p.command(script),
        }
    }
}

fn fail(e: player::PlayerError) -> i32 {
    let (msg, code) = cli::report(e);
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
        println!("{}", cli::render_json(&state));
    } else {
        print!("{}", cli::render_status(&state, style));
    }
    EXIT_OK
}

/// `trak play <something>`.
///
/// A Spotify URI plays straight away through AppleScript, which works on the Free
/// tier. A *name* needs search, which needs the Web API and therefore a Client
/// ID. Without one this says how to get a Client ID and exits 2 rather than
/// pretending to have played something (SPEC §9, TODO 7.12).
fn play(player: &AnyPlayer, target: &str) -> i32 {
    let t = target.trim();
    // A URI is validated here, at the one point a user-supplied string enters
    // trak, so every backend enforces the same rule and none can be bypassed.
    if t.starts_with("spotify:") || t.contains("://") {
        if let Err(e) = crate::player::check_playable_uri(t) {
            let (msg, code) = cli::report(e);
            eprintln!("{msg}");
            return code;
        }
        return match player.play_uri(t) {
            Ok(()) => EXIT_OK,
            Err(e) => fail(e),
        };
    }

    // Search half, not built yet (TODO 7.12). shpotify needed a Client ID for this
    // too, so the behaviour is the same: explain, then exit 2.
    eprintln!(
        "`trak play \"{t}\"` needs to search Spotify, which needs a Client ID.\n\n\
         One-time setup:\n\
           1. open https://developer.spotify.com/dashboard\n\
           2. create an app, and add the redirect URI  http://127.0.0.1\n\
           3. run `trak config` and paste the Client ID\n\n\
         Until then you can play a URI directly:\n\
           trak play uri spotify:track:6HacgXCExkzS552ILfJTXu"
    );
    EXIT_USAGE
}

/// `stop`. Spotify's dictionary has no `stop` command — `tell ... to stop`
/// silently no-ops — so this is shpotify's behaviour: pause if playing, otherwise
/// say so (docs/APPLESCRIPT.md §6).
fn stop(player: &mut AnyPlayer) -> i32 {
    match player.state() {
        Ok(s) if s.playback == player::PlaybackState::Playing => {
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
    if player.real().is_none() {
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
                if s.playback == player::PlaybackState::Paused {
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
        Some(VolArg::Up) => cli::run_volume(player, VOLUME_STEP as i16, json),
        Some(VolArg::Down) => cli::run_volume(player, -(VOLUME_STEP as i16), json),
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
        // Without the remembered mode the `r` key would never come back round.
        Mode::Repeat => {
            let next = player.repeat_mode().next();
            player.set_repeat_mode(next);
            format!("set repeating to {}", next != player::RepeatMode::Off)
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
    if let Ok(mut pbcopy) = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        use std::io::Write;
        let _ = pbcopy
            .stdin
            .take()
            .map(|mut s| s.write_all(link.as_bytes()));
    }
    EXIT_OK
}
