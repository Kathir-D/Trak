//! The event loop, and the terminal it owns.
//!
//! The only place in the TUI that touches the terminal or a player. Everything
//! else is pure, which is why the app and the renderer can be tested without
//! either (ARCHITECTURE, "Never block the UI thread").
//!
//! The hard requirement from TODO 3.1: **quitting must leave the terminal
//! usable**, including after a panic. So the alternate screen, raw mode and the
//! cursor are restored from a panic hook as well as on the happy path.

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use crossterm::ExecutableCommand;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind, poll, read,
};
use crossterm::terminal::{
    DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::art;
use crate::player::AppleScriptPlayer;
use crate::player::PlayerCommand;
use crate::player::actions::Worker;
use crate::player::actions::{CommandOutcome, WorkerResult, WriteOutcome};
use crate::tui::app::{App, Event, update};
use crate::tui::theme::{Accent, Border, Theme};

/// How long to wait for a terminal event before doing anything else.
///
/// This is also the refresh rate of the clock and the progress interpolation.
/// It is short because the bar is interpolated locally from the last read, so
/// moving it costs nothing and an AppleScript read never happens at this rate
/// (docs/APPLESCRIPT.md §4).
const INPUT_WAIT: Duration = Duration::from_millis(100);

/// The poll interval. The notification (TODO 3.9) is the primary update path;
/// this is only the safety net, so it is deliberately slow. An AppleScript read is
/// ~430 ms and covers only what the notification is silent about
/// (docs/APPLESCRIPT.md §4 and §9).
/// How long a cell of a scrolling title stays put. Slow enough to read a word at a
/// time; fast enough that the title does not feel stuck.
const SCROLL_EVERY: Duration = Duration::from_millis(220);

/// How often Sonar's state file is re-read. COMPAT's test matrix calls this
/// "fresh, every few seconds"; a duck lasts a few seconds, so anything slower
/// would show the badge after the duck had finished.
const SONAR_EVERY: Duration = Duration::from_secs(2);

const POLL_PLAYING: Duration = Duration::from_secs(3);
const POLL_IDLE: Duration = Duration::from_secs(5);

/// Owns the terminal, and puts it back no matter how we leave.
pub struct TerminalGuard {
    restored: bool,
}

impl TerminalGuard {
    /// Enter the alternate screen and raw mode.
    ///
    /// The panic hook is installed first, so a panic anywhere below still leaves
    /// the user's terminal readable rather than stuck in raw mode.
    pub fn enter() -> std::io::Result<Self> {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = restore_terminal();
            previous(info);
        }));

        enable_raw_mode()?;
        let mut out = std::io::stdout();
        // Line wrap off: a stray wide glyph must not reflow the whole screen.
        let _ = out.execute(DisableLineWrap);
        out.execute(EnterAlternateScreen)?;
        Ok(Self { restored: false })
    }

    /// Put the terminal back. Safe to call more than once.
    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let _ = restore_terminal();
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

fn restore_terminal() -> std::io::Result<()> {
    let mut out = std::io::stdout();
    let _ = out.execute(crossterm::cursor::Show);
    let _ = out.execute(LeaveAlternateScreen);
    let _ = out.execute(EnableLineWrap);
    let _ = disable_raw_mode();
    out.flush()
}

/// Run the TUI. Returns the process exit code.
pub fn run() -> i32 {
    // A TUI needs a terminal. Without one, entering raw mode and the alternate
    // screen produces an unreadable mess and a process that looks hung, so say
    // so and point at the commands that do work.
    if !std::io::stdout().is_terminal() {
        eprintln!(
            "trak: the TUI needs a terminal.\n\
             For a one-shot command use `trak status`, `trak vol up`, `trak next`, or `trak --help`."
        );
        return 2;
    }

    let mut guard = match TerminalGuard::enter() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("trak: cannot start the TUI: {e}");
            return 1;
        }
    };

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = match Terminal::new(backend) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("trak: cannot start the TUI: {e}");
            guard.restore();
            return 1;
        }
    };
    // Mouse is on by default (SPEC §2, `[input] mouse`); it makes the progress
    // bar seekable and the tabs clickable (TODO 3.8). It is only *asked for* when
    // the setting is on, so a terminal that reports mouse events cannot steal
    // text selection from a user who turned it off.
    let settings = crate::tui::app::Settings::default();
    if settings.mouse {
        let _ = terminal.backend_mut().execute(EnableMouseCapture);
    }
    let _ = terminal.clear();

    let code = event_loop(&mut terminal, settings);

    let _ = terminal.backend_mut().execute(DisableMouseCapture);
    guard.restore();
    code
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    settings: crate::tui::app::Settings,
) -> i32 {
    let worker = Worker::new(AppleScriptPlayer::new());
    // The notification is the fast path; the poll stays as the safety net for
    // seek / volume / shuffle / repeat, which it is silent about
    // (docs/APPLESCRIPT.md section 9). If it cannot be registered, trak carries
    // on with the poll alone rather than failing.
    //
    // Must be called on the main thread, because that is where
    // NSDistributedNotificationCenter delivers -- a background run loop receives
    // nothing at all, which cost a confusing debugging round to find out.
    let notify = crate::player::notify::subscribe();
    let mut app = App::new();
    app.settings = settings;
    // The accent comes off the cover, so the theme is mutable for the life of the
    // session (TODO 4.2).
    let mut theme = Theme::new(Accent::Art, Border::Rounded);
    // Where the last frame put the clickable things, and whether a seek drag is
    // in progress. A drag keeps seeking after the pointer leaves the bar, which
    // is the whole point of being able to scrub.
    let mut regions = crate::tui::render::Regions::default();
    // The protocol is negotiated once, here, after the alternate screen is up and
    // before any event is read (TODO 1.4's ordering requirement).
    //
    let mut images = crate::tui::render::Images::from_terminal();
    // Ask headless-spotify once, at startup, rather than per frame (TODO 4.7).
    if crate::headless::is_installed() && !worker.is_busy() {
        worker.submit(|_| WorkerResult::Headless(headless_status()));
    }
    let mut scrubbing = false;

    // `None` means "never polled", which is due straight away. Starting the
    // clock at `now` instead would leave the TUI sitting on the idle card for a
    // whole poll interval before it found out Spotify was running.
    let mut last_poll: Option<Instant> = None;
    let mut last_clock_tick = Instant::now();
    let mut last_scroll = Duration::ZERO;
    let mut last_sonar = Instant::now() - SONAR_EVERY;

    loop {
        // 1. Finished writes first, so a completed command is applied before the
        //    next key can queue another.
        while let Some(result) = worker.poll() {
            app = match result {
                WorkerResult::State(s) => update(app, Event::PlayerState(s)).app,
                WorkerResult::ReadFailed(crate::player::PlayerError::NotRunning) => {
                    update(app, Event::NotRunning).app
                }
                // A read that failed for any other reason is not a reason to
                // throw away what is already on screen; show it as a notice.
                WorkerResult::ReadFailed(e) => {
                    let (msg, _) = crate::cli::report(e);
                    let mut next = app;
                    next.toast = Some(crate::tui::app::Toast {
                        text: msg.lines().next().unwrap_or("read failed").to_string(),
                        at: std::time::Instant::now(),
                    });
                    next
                }
                WorkerResult::Command(outcome) => update(app, Event::CommandDone(outcome)).app,
                WorkerResult::Sonar(state) => update(app, Event::Sonar(state)).app,
                WorkerResult::Headless(h) => update(app, Event::Headless(h)).app,
                WorkerResult::Lyrics { uri, result } => {
                    update(app, Event::Lyrics { uri, result }).app
                }
                WorkerResult::Art { url, result } => {
                    let u = update(app, Event::Art { url, result });
                    // Take the accent from the cover that just arrived, if there
                    // is one and it has a usable colour. The pixels are already
                    // in the app; the theme needs a look at them.
                    if let Some(art) = u.app.art.loaded.as_ref()
                        && theme.set_art_colour(&art.image)
                    {
                        // A new accent means every cell that was the old colour
                        // has to be repainted, so the frame is forced rather than
                        // left to the diff.
                        terminal.clear().ok();
                    }
                    u.app
                }
            };
        }

        // 2. A notification, if one arrived. This is what makes a skip from
        //    Sonar or a media key show up in about 170ms instead of on the next
        //    poll. It carries no artwork, so the merge keeps the existing cover
        //    and the volume it already knew.
        if let Some(sub) = &notify
            && let Some(event) = sub.poll()
        {
            if let Some(state) = app.state.as_ref() {
                let track_changed = event.is_different_track(state);
                let merged = crate::player::notify::merge(state, &event);
                app = update(app, Event::PlayerState(Box::new(merged))).app;

                // A track change is the one thing the notification cannot be
                // trusted about on its own: it has no artwork, play count or
                // popularity, so one read has to follow. It is cheap, because
                // that is once per song rather than once per poll.
                if track_changed {
                    worker.submit(|p| match p.state() {
                        Ok(st) => crate::player::actions::WorkerResult::State(Box::new(st)),
                        Err(e) => crate::player::actions::WorkerResult::ReadFailed(e),
                    });
                }
            }
        }

        // 2b. Album art (TODO 4.1). One download per track, on the worker, and
        //     only when the track has artwork we do not already have. The cache
        //     makes a rewind through the history free.
        if app.art_enabled
            && let Some(track) = app.track()
            && let Some(url) = track.artwork_url.clone()
            && app.art.wants(track)
            && app.art.begin(&url)
        {
            // `begin` above already claimed the slot, so a refused submission
            // has to be undone: otherwise the art is "loading" for the rest of
            // the session with nothing on its way.
            let accepted = worker.submit(move |_| {
                use crate::player::actions::{LoadedArt, WorkerResult};
                // The result is a decoded image or a reason, not a PlayerError: a
                // missing cover is not a Spotify failure.
                let result = art::fetch(&url)
                    .and_then(|path| art::decode(&path).map(|image| LoadedArt { path, image }));
                WorkerResult::Art { url, result }
            });
            if !accepted {
                app.art.abandon();
            }
        }

        // 2c. Lyrics (TODO 6.1). One lookup per track, on the worker, and only
        //     when they are switched on. An advert is skipped outright: it is not
        //     a song and LRCLIB will not have heard of it.
        if app.settings.lyrics
            && app.lyrics.status == crate::tui::app::LyricsStatus::Idle
            && let Some(track) = app.track()
        {
            if track.is_ad() {
                app.lyrics.status = crate::tui::app::LyricsStatus::NotFound;
            } else {
                let uri = track.uri.clone();
                let title = track.title.clone();
                let artist = track.artist.clone();
                let album = track.album.clone();
                let dur = track.duration_secs();
                app.lyrics.uri = uri.clone();
                app.lyrics.status = crate::tui::app::LyricsStatus::Loading;
                worker.submit(move |_| {
                    let album = (!album.is_empty()).then_some(album);
                    let result = crate::lyrics::fetch(&title, &artist, album.as_deref(), Some(dur));
                    WorkerResult::Lyrics { uri, result }
                });
            }
        }

        // 2d. Sonar's state file (TODO 4.6). Read on a timer rather than per
        //     frame: it is a small file and the answer changes on the order of
        //     seconds, while a frame is 100 ms. Reading it more often than that
        //     would be a way of making the disk busy for no information.
        if last_sonar.elapsed() >= SONAR_EVERY && !worker.is_busy() {
            last_sonar = Instant::now();
            worker.submit(|_| WorkerResult::Sonar(sonar_state()));
        }

        // 3. Terminal input. The wait is a run-loop pump, not a sleep:
        //    NSDistributedNotificationCenter only delivers on the main run loop,
        //    so a plain sleep here would leave the observer registered and silent.
        match poll(Duration::ZERO) {
            Ok(true) => match read() {
                Ok(TermEvent::Key(k)) => {
                    if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
                        app = update(app, Event::Quit).app;
                    } else if let Some(c) = char_for(k) {
                        let u = update(app, Event::Key(c));
                        app = u.app;
                        submit_all(u.commands, &worker);
                    }
                }
                Ok(TermEvent::Mouse(m)) => {
                    if app.settings.mouse {
                        let u = update(app, mouse_event(m, &regions, &mut scrubbing));
                        app = u.app;
                        submit_all(u.commands, &worker);
                    }
                }
                Ok(TermEvent::Resize(_, _)) => {
                    app = update(app, Event::Resize).app;
                    // Kitty's encoded state is only valid for the size it was
                    // encoded at, so a resize has to throw it away or the art is
                    // drawn at the wrong size until the next track.
                    images.invalidate();
                    // ratatui handles the buffer; a redraw picks the new size up.
                }
                Ok(_) => {}
                Err(e) => {
                    // A broken stdin means there is no terminal to talk to.
                    eprintln!("trak: terminal input failed: {e}");
                    break;
                }
            },
            Ok(false) => {}
            Err(e) => {
                eprintln!("trak: terminal input failed: {e}");
                break;
            }
        }
        // Waiting happens here rather than inside `poll`, so the run loop gets
        // the time instead of the terminal read. Together they pace the frame.
        crate::player::notify::pump_run_loop(INPUT_WAIT.as_secs_f64());

        // 4. The local tick: toast expiry and the poll-due flag.
        app = update(app, Event::Tick).app;

        if last_clock_tick.elapsed() >= Duration::from_secs(1) {
            last_clock_tick = Instant::now();
            app.clock = clock_string();
        }

        // 5. A poll when one is due. `poll_due` is false while a command is in
        //    flight, so a poll never queues behind a write.
        let interval = if app.is_playing() {
            POLL_PLAYING
        } else {
            POLL_IDLE
        };
        let due = last_poll.is_none_or(|t| t.elapsed() >= interval);
        if app.poll_due && !worker.is_busy() && due {
            last_poll = Some(Instant::now());
            worker.submit(|p| match p.state() {
                Ok(st) => WorkerResult::State(Box::new(st)),
                Err(e) => WorkerResult::ReadFailed(e),
            });
        }

        // 6. Draw, and keep the clickable regions from this frame. The next
        //    click is resolved against what is on screen now, not against a
        //    second copy of the layout.
        if terminal
            .draw(|f| crate::tui::render::draw_with(f, &app, &theme, &mut regions, &mut images))
            .is_err()
        {
            break;
        }

        // 7. Tell the app how many history rows fit, whenever that changes, so
        //    the view scrolls to follow the cursor.
        if let Some(rows) = regions.history_rows()
            && rows != app.viewport
        {
            app = update(app, Event::Viewport(rows)).app;
        }

        // 8. A title too long for the pane scrolls. The renderer knows whether it
        //    fits -- only it knows the pane width -- so it reports the span and
        //    this owns the clock. A title that fits never moves.
        if regions.title_span > 0 {
            last_scroll += Duration::from_millis(100);
            if last_scroll >= SCROLL_EVERY {
                last_scroll = Duration::ZERO;
                app.marquee_offset = (app.marquee_offset + 1) % regions.title_span;
            }
        }
        if app.should_quit {
            break;
        }
    }

    // The worker shuts itself down and joins its thread on drop, so no thread is
    // left holding an AppleScript call when the terminal is restored. TODO 8.5
    // extends that to proving the process tap leaves no device behind.
    0
}

/// Queue commands on the worker.
fn submit_all(commands: Vec<PlayerCommand>, worker: &Worker) {
    for cmd in commands {
        worker.submit(move |p| {
            let result = run_one(p, cmd.clone());
            WorkerResult::Command(match result {
                // The read-back travels with the result rather than being thrown
                // away: a volume write Spotify ignored is a state the app has to
                // hear about (COMPAT rule 5), not a success.
                Ok(Some(outcome)) => CommandOutcome::read_back(cmd, outcome),
                Ok(None) => CommandOutcome::ok(cmd),
                Err(e) => CommandOutcome::failed(cmd, e),
            })
        });
    }
}

/// Run one write. `Ok(Some(outcome))` means the command was read back.
fn run_one(
    p: &mut dyn crate::player::Player,
    cmd: PlayerCommand,
) -> Result<Option<WriteOutcome>, crate::player::PlayerError> {
    use crate::player::actions::{seek_checked, set_volume_checked, step_volume};

    match cmd {
        PlayerCommand::Toggle => p.toggle().map(|_| None),
        PlayerCommand::Next => p.next().map(|_| None),
        PlayerCommand::Prev => p.previous().map(|_| None),
        PlayerCommand::Replay => p.seek(0.0).map(|_| None),
        PlayerCommand::Seek(secs) => seek_checked(p, secs).map(Some),
        PlayerCommand::VolumeStep(step) => step_volume(p, step).map(Some),
        PlayerCommand::SetVolume(v) => set_volume_checked(p, v).map(Some),
        PlayerCommand::ToggleShuffle => {
            let on = !p.state()?.shuffling_enabled;
            p.command(&format!("set shuffling to {on}")).map(|_| None)
        }
        PlayerCommand::CycleRepeat => {
            // AppleScript cannot read back "repeat one" as distinct from "repeat
            // all", so the app remembers the mode and this writes its boolean.
            let on = !p.state()?.repeating_enabled;
            p.command(&format!("set repeating to {on}")).map(|_| None)
        }
        // `enter` on a history row (TODO 3.6). The URI came out of a read, and
        // `play_uri` checks the allow-list itself, so there is no path from a
        // Spotify string to a generated AppleScript literal.
        PlayerCommand::PlayUri(uri) => p.play_uri(&uri).map(|_| None),
        // Copying is not a Spotify write at all, so it never touches a player.
        PlayerCommand::CopyLink(link) => {
            // A pasteboard that refuses is not a Spotify failure, and the link is
            // already in the toast, so there is nothing to report. Deliberately
            // not an error: a failed copy must never look like a failed command.
            let _ = crate::player::copy_to_clipboard(&link);
            Ok(None)
        }
        PlayerCommand::Launch => launch_spotify().map(|_| None),
    }
}

/// Read Sonar's state file, or `unknown` when there is nothing usable there.
///
/// `read_state_trusted` is what trak uses: it vets the file against *this* Spotify's
/// process id, so a state file left behind by a Sonar that has since been
/// restarted -- or by one watching a different Spotify -- is not believed.
/// Run `headless-spotify status --json` once and read the answer.
///
/// A failure is not an error worth a toast: the sibling may be absent, may be a
/// version trak does not understand, or may simply not be running. `unknown()`
/// is the right answer to all three.
fn headless_status() -> crate::headless::Headless {
    let out = std::process::Command::new(crate::headless::PROGRAM)
        .args(["status", "--json"])
        .output();
    match out {
        Ok(o) => crate::headless::Headless::from_status(true, &o.stdout),
        Err(_) => crate::headless::Headless::unknown(),
    }
}

fn sonar_state() -> crate::sonar::SonarState {
    let path = sonar_state_path();
    let spotify_pid = std::process::Command::new("/bin/pgrep")
        .args(["-x", "Spotify"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .and_then(|l| l.trim().parse().ok())
        });
    crate::sonar::read_state_trusted(
        &path,
        crate::sonar::Trust::Checked {
            spotify_pid,
            sonar_alive: true,
        },
    )
}

/// Where Sonar is expected to leave its state. `SONAR_STATE` wins so the tests
/// and a second instance can point it elsewhere; COMPAT's documented default is
/// under the user's Library.
fn sonar_state_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("SONAR_STATE") {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    home.join("Library")
        .join("Application Support")
        .join("Sonar")
        .join("state.json")
}

/// Turn a crossterm mouse event into one the app can act on.
///
/// The hit test uses the regions the previous frame recorded. While a drag is in
/// progress the target is the progress bar whatever the pointer is over, because
/// a scrub that stopped the moment the pointer left the bar would be useless.
fn mouse_event(
    m: MouseEvent,
    regions: &crate::tui::render::Regions,
    scrubbing: &mut bool,
) -> Event {
    use crate::tui::app::{Hit, Mouse, MouseAction};

    let action = match m.kind {
        MouseEventKind::Down(MouseButton::Left) => MouseAction::Press,
        MouseEventKind::Drag(MouseButton::Left) => MouseAction::Drag,
        MouseEventKind::Up(MouseButton::Left) => {
            *scrubbing = false;
            return Event::Tick;
        }
        MouseEventKind::ScrollUp => MouseAction::ScrollUp,
        MouseEventKind::ScrollDown => MouseAction::ScrollDown,
        // A middle click, a right click, a double click or a mouse move: trak
        // has no use for any of them, and inventing one now would be a decision
        // the SPEC does not cover.
        _ => return Event::Tick,
    };

    let target = match regions.hit(m.column, m.row) {
        Some(Hit::Seek(f)) => {
            *scrubbing = true;
            Hit::Seek(f)
        }
        // Mid-drag, off the bar: keep seeking rather than dropping the scrub.
        Some(_) if *scrubbing && action == MouseAction::Drag => Hit::Seek(
            regions
                .progress
                .filter(|p| p.width > 0)
                .map(|p| {
                    ((m.column.saturating_sub(p.x) as f64 + 0.5) / p.width as f64).clamp(0.0, 1.0)
                })
                .unwrap_or(0.0),
        ),
        Some(other) => {
            if *scrubbing {
                Hit::Seek(0.0)
            } else {
                other
            }
        }
        None => {
            *scrubbing = false;
            return Event::Tick;
        }
    };

    Event::Mouse(Mouse { action, target })
}

/// The char `update` should see for a key, or `None` for a key trak ignores.
///
/// Arrows are folded onto their vim equivalents so both work from one binding
/// (SPEC §4: "Arrows **and** vim keys").
fn char_for(k: KeyEvent) -> Option<char> {
    // Some terminals emit a release event as well as a press; acting on both would
    // make a held key repeat twice as fast.
    if k.kind == KeyEventKind::Release {
        return None;
    }
    match k.code {
        KeyCode::Char(c) if matches!(c, 'h' | 'j' | 'k' | 'l') => Some(c),
        KeyCode::Char(c) => Some(c),
        KeyCode::Enter => Some('\n'),
        KeyCode::Tab if k.modifiers.contains(KeyModifiers::SHIFT) => Some('Z'),
        KeyCode::Tab => Some('\t'),
        // The escape character itself, so `update` can bind it as a real key
        // rather than the loop growing a special case for it (SPEC §4: `esc`
        // closes the overlay).
        KeyCode::Esc => Some('\x1b'),
        KeyCode::Left => Some('h'),
        KeyCode::Right => Some('l'),
        KeyCode::Down => Some('j'),
        KeyCode::Up => Some('k'),
        _ => None,
    }
}

/// Ask Launch Services to start Spotify in the background: no focus, no Dock
/// bounce. This is the single launch trak ever performs (COMPAT rule 2), and only
/// because the user pressed enter on the idle card.
fn launch_spotify() -> Result<(), crate::player::PlayerError> {
    // `headless-spotify launch` when the sibling is installed (TODO 4.7): it is
    // the supported way to start Spotify without a Dock icon, and COMPAT rule 2
    // asks for exactly that.
    crate::headless::launch_command()
        .spawn()
        .map(|_| ())
        .map_err(|e| crate::player::PlayerError::Script(format!("launching Spotify: {e}")))
}

fn clock_string() -> String {
    // The header only needs HH:MM, and `date` is on every macOS, so there is no
    // reason to take a date dependency for it.
    std::process::Command::new("date")
        .arg("+%H:%M")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: mods,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn plain_characters_pass_through() {
        assert_eq!(
            char_for(key(KeyCode::Char('n'), KeyModifiers::NONE)),
            Some('n')
        );
        assert_eq!(
            char_for(key(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some('q')
        );
    }

    #[test]
    fn enter_and_tab_become_control_chars() {
        assert_eq!(
            char_for(key(KeyCode::Enter, KeyModifiers::NONE)),
            Some('\n')
        );
        assert_eq!(char_for(key(KeyCode::Tab, KeyModifiers::NONE)), Some('\t'));
    }

    /// SPEC §4: arrows and vim keys both work, from one binding.
    #[test]
    fn arrows_fold_onto_their_vim_equivalents() {
        assert_eq!(char_for(key(KeyCode::Left, KeyModifiers::NONE)), Some('h'));
        assert_eq!(char_for(key(KeyCode::Right, KeyModifiers::NONE)), Some('l'));
        assert_eq!(char_for(key(KeyCode::Down, KeyModifiers::NONE)), Some('j'));
        assert_eq!(char_for(key(KeyCode::Up, KeyModifiers::NONE)), Some('k'));
    }

    /// SPEC §4: `esc` closes the overlay, so the loop has to hand it to `update`
    /// as the character it binds.
    #[test]
    fn the_escape_key_reaches_update() {
        assert_eq!(
            char_for(key(KeyCode::Esc, KeyModifiers::NONE)),
            Some('\x1b')
        );
    }

    /// Acting on a release as well as a press would double every keypress.
    #[test]
    fn key_release_events_are_ignored() {
        let mut k = key(KeyCode::Char('n'), KeyModifiers::NONE);
        k.kind = KeyEventKind::Release;
        assert_eq!(char_for(k), None);
    }

    #[test]
    fn ctrl_c_is_routed_through_the_same_quit_path() {
        // The loop must not poke should_quit directly, or the quit logic ends up
        // in two places.
        let app = update(App::new(), Event::Quit).app;
        assert!(app.should_quit);
    }

    #[test]
    fn a_key_trak_has_no_binding_for_is_ignored() {
        assert_eq!(char_for(key(KeyCode::F(5), KeyModifiers::NONE)), None);
        assert_eq!(char_for(key(KeyCode::Home, KeyModifiers::NONE)), None);
    }

    #[test]
    fn the_clock_is_hh_mm_or_empty() {
        let c = clock_string();
        assert!(c.is_empty() || (c.len() == 5 && c.contains(':')), "{c:?}");
    }

    /// COMPAT rule 2: the launch must not focus Spotify or bounce the Dock. This
    /// asserts the exact argv of *both* branches, because a test that only checks
    /// "the command exists" cannot tell a correct `-g -j` from a plain `open`.
    #[test]
    fn the_launch_command_is_backgrounded_in_both_branches() {
        let headless = crate::headless::launch_command_for(true);
        assert_eq!(headless.get_program(), "headless-spotify");
        assert_eq!(collect_args(&headless), vec!["launch"]);

        let plain = crate::headless::launch_command_for(false);
        assert_eq!(plain.get_program(), "open");
        assert_eq!(collect_args(&plain), vec!["-g", "-j", "-a", "Spotify"]);
    }

    fn collect_args(cmd: &std::process::Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn every_command_runs_against_the_fake_without_panicking() {
        use crate::player::FakePlayer;
        let mut p = FakePlayer::playing();
        for cmd in [
            PlayerCommand::Toggle,
            PlayerCommand::Next,
            PlayerCommand::Prev,
            PlayerCommand::Replay,
            PlayerCommand::Seek(30.0),
            PlayerCommand::VolumeStep(10),
            PlayerCommand::VolumeStep(-10),
            PlayerCommand::SetVolume(50),
            PlayerCommand::PlayUri("spotify:track:6HacgXCExkzS552ILfJTXu".into()),
        ] {
            // The fake has no AppleScript `command`, so the two that need it are
            // checked separately; everything else must simply not panic.
            if run_one(&mut p, cmd.clone()).is_err() {
                assert!(
                    matches!(
                        cmd,
                        PlayerCommand::ToggleShuffle | PlayerCommand::CycleRepeat
                    ),
                    "only the guarded writes may fail on the fake, got {cmd:?}"
                );
            }
        }
    }

    /// The History tab's `enter`, end to end against the fake (TODO 3.6).
    #[test]
    fn enter_on_a_history_row_reaches_spotify() {
        use crate::player::{FakePlayer, PlaybackState, Player};
        let mut p = FakePlayer::playing();
        let before = p.state().unwrap().track.uri.clone();
        run_one(
            &mut p,
            PlayerCommand::PlayUri("spotify:track:replayed".into()),
        )
        .unwrap();
        let s = p.state().unwrap();
        assert_ne!(s.track.uri, before);
        assert_eq!(s.track.uri.as_deref(), Some("spotify:track:replayed"));
        assert_eq!(s.playback, PlaybackState::Playing);
    }

    /// The URI is interpolated into an AppleScript literal, so one that is not on
    /// the allow-list must never get that far.
    #[test]
    fn an_unplayable_uri_is_refused_before_it_reaches_spotify() {
        use crate::player::{FakePlayer, Player};
        let mut p = FakePlayer::playing();
        let before = p.state().unwrap().track.uri.clone();
        for bad in [
            "spotify:ad:1",
            "spotify:track:x\" & (do shell script \"id\") & \"",
            "https://example.com",
        ] {
            assert!(
                run_one(&mut p, PlayerCommand::PlayUri(bad.into())).is_err(),
                "{bad:?} must be refused"
            );
        }
        assert_eq!(
            p.state().unwrap().track.uri,
            before,
            "the player was left alone"
        );
    }
}
