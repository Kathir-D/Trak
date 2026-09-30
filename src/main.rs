//! trak — an interactive terminal UI for the Spotify desktop app on macOS.
//!
//! `trak` on its own opens the TUI. Every other subcommand is a one-shot
//! command, kept compatible with shpotify (SPEC §9).

mod cli;
mod player;
#[cfg(test)]
mod testutil;

use std::io::IsTerminal as _;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::cli::{EXIT_FAIL, EXIT_OK, EXIT_USAGE, Style};
use crate::player::Player;
use crate::player::applescript::AppleScriptPlayer;

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
    let cli = Cli::parse();
    let mut player = AppleScriptPlayer::new();

    let code = match cli.command {
        // Bare `trak` is the TUI, which does not exist yet (TODO 3.1). It must
        // exist as a subcommand-free path now, printing something honest rather
        // than doing nothing (TODO 2.8).
        None => {
            eprintln!(
                "trak {} — the TUI is not built yet (TODO 3.1).\n\
                 Meanwhile: `trak status`, `trak vol up`, `trak next`, `trak --help`.",
                env!("CARGO_PKG_VERSION")
            );
            EXIT_USAGE
        }
        Some(Command::Status { field }) => status(&player, cli.json, style(cli.plain), field),
        Some(Command::Play { uri: Some(uri) }) => match player.play_uri(&uri) {
            Ok(()) => EXIT_OK,
            Err(e) => fail(e),
        },
        Some(Command::Play { uri: None }) => cli::run_action(&mut player, "play"),
        Some(Command::Pause) => cli::run_action(&mut player, "toggle"),
        Some(Command::Stop) => stop(&mut player),
        Some(Command::Quit) => quit(&mut player),
        Some(Command::Next) => cli::run_action(&mut player, "next"),
        Some(Command::Prev) => cli::run_action(&mut player, "prev"),
        Some(Command::Replay) => replay(&mut player),
        Some(Command::Pos { secs }) => match player.seek(secs) {
            Ok(()) => EXIT_OK,
            Err(e) => fail(e),
        },
        Some(Command::Vol { arg }) => vol(&mut player, arg, cli.json),
        Some(Command::Toggle { what }) => mode(&mut player, what),
        Some(Command::Share { what }) => share(&player, what),
    };

    ExitCode::from(code as u8)
}

fn fail(e: player::PlayerError) -> i32 {
    let (msg, code) = cli::report(e);
    eprintln!("{msg}");
    code
}

fn status(player: &AppleScriptPlayer, json: bool, style: Style, field: Option<Field>) -> i32 {
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

/// `stop`. Spotify's dictionary has no `stop` command — `tell ... to stop`
/// silently no-ops — so this is shpotify's behaviour: pause if playing, otherwise
/// say so (docs/APPLESCRIPT.md §6).
fn stop(player: &mut AppleScriptPlayer) -> i32 {
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
fn quit(player: &mut AppleScriptPlayer) -> i32 {
    match player.command("quit") {
        Ok(()) => {
            println!("Quitting Spotify.");
            EXIT_OK
        }
        Err(e) => fail(e),
    }
}

fn replay(player: &mut AppleScriptPlayer) -> i32 {
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

fn vol(player: &mut AppleScriptPlayer, arg: Option<VolArg>, json: bool) -> i32 {
    let current = match player.state() {
        Ok(s) => s.volume,
        Err(e) => return fail(e),
    };

    let target = match arg {
        None | Some(VolArg::Show) => {
            if json {
                println!("{{\"volume\":{current}}}");
            } else {
                println!("{current}");
            }
            return EXIT_OK;
        }
        Some(VolArg::Up) => current.saturating_add(VOLUME_STEP),
        Some(VolArg::Down) => current.saturating_sub(VOLUME_STEP),
    };

    if let Err(e) = player.set_volume(target) {
        return fail(e);
    }

    // COMPAT rule 5: read back, and allow ±1, or every write looks like a failure.
    let read = match player.state() {
        Ok(s) => s.volume,
        Err(e) => return fail(e),
    };
    if read.abs_diff(target) > 1 {
        eprintln!(
            "Spotify ignored the volume change (asked for {target}, it reports {read}). \
             Hiding the volume meter; see COMPAT rule 5."
        );
        return EXIT_FAIL;
    }
    if json {
        println!("{{\"volume\":{read}}}");
    } else {
        println!("{read}");
    }
    EXIT_OK
}

fn mode(player: &mut AppleScriptPlayer, what: Mode) -> i32 {
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

fn share(player: &AppleScriptPlayer, what: ShareArg) -> i32 {
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
