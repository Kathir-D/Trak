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
use crate::tui::app::{Tab, WebJob};
use crate::tui::theme::Theme;
use crate::web::api::{Library, SpotifyLibrary};
use crate::web::token::Store;

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

/// The frame rate the visualizer is drawn at (TODO 8.4). 30 fps is where bars
/// stop looking like a slideshow; above that they cost battery and look the same.
const VIZ_FPS: Duration = Duration::from_millis(33);

/// Shown once, on a machine that has never run trak (TODO 5.4). It has to say
/// where the settings are and what the optional bit is, because a first launch
/// is the only moment a user is guaranteed to be looking.
const FIRST_RUN_HINT: &str = "press , for settings \u{b7} optionally add a Spotify Client ID there for search and playlists (press any key to dismiss)";

/// Whether the visualizer is the thing on screen right now. The frame rate, the
/// fetch and the tap all hang off this rather than off the setting, so hiding
/// the visualizer really does stop the work (TODO 8.5).
fn visualizer_visible(app: &crate::tui::app::App) -> bool {
    app.settings.display_mode == crate::tui::app::DisplayMode::Visualizer && !app.is_idle()
}

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
    // The config file is read before the alternate screen is cleared, so a
    // corrupt file's notice has somewhere to go and the first frame is already
    // drawn with the user's settings rather than flashing the defaults.
    let loaded = crate::config::Config::load();
    let (config, notice, first_run) = match loaded {
        Ok(l) => {
            // A missing file is the first run, not an error, and it is the only
            // thing that is different about it -- so it is a fact to remember
            // rather than a state to model (TODO 5.4).
            let first_run = !l.path.exists();
            (Some(l), None, first_run)
        }
        Err(e) => (None, Some(e.notice().to_string()), false),
    };
    let settings = config
        .as_ref()
        .map(|l| l.config.settings())
        .unwrap_or_default();
    if settings.mouse {
        let _ = terminal.backend_mut().execute(EnableMouseCapture);
    }
    let _ = terminal.clear();

    let (code, pending) = event_loop(&mut terminal, settings, config, notice, first_run);

    let _ = terminal.backend_mut().execute(DisableMouseCapture);
    guard.restore();

    // Saved after the terminal is back, so a failure is a line of text rather
    // than an escape sequence. Losing a setting is worth saying out loud; it is
    // not worth a panic on the way out of a TUI.
    if let Some(config) = pending
        && let Err(e) = config.save()
    {
        eprintln!("trak: could not write the config file: {}", e.notice());
    }
    code
}

/// `trak config`: the settings screen on its own, with no dashboard behind it
/// (TODO 2.8, 5.2).
///
/// It is the same screen and the same code as the TUI's `,` -- one screen, two
/// ways in -- so a setting cannot be editable in one and not the other. The
/// difference is only what is behind it: here there is no track, no cover and no
/// poll, which is why it works before Spotify has ever been launched.
pub fn config_screen() -> i32 {
    if !std::io::stdout().is_terminal() {
        eprintln!(
            "trak: `trak config` needs a terminal.\n\
             The settings file is plain TOML: {}",
            crate::config::Config::default()
                .to_toml()
                .lines()
                .map(|l| format!("  {l}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        return 2;
    }

    let loaded = crate::config::Config::load();
    let mut app = App::new();
    let mut pending: Option<String> = None;
    match loaded {
        Ok(l) => {
            if let Some(n) = l.notice() {
                pending = Some(n);
            }
            app.config = l.config.clone();
            app.settings = l.config.settings();
        }
        Err(e) => {
            app.toast(e.notice());
        }
    }
    app.settings_open = true;
    crate::tui::settings::open(&mut app);
    app.web.connection = connection_at_start(&app.config.spotify.client_id);
    let mut setup = SetupRunner::default();
    let depth = crate::tui::colour::Depth::from_env();

    let mut guard = match TerminalGuard::enter() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("trak: cannot start the settings screen: {e}");
            return 1;
        }
    };
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = match Terminal::new(backend) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("trak: cannot start the settings screen: {e}");
            guard.restore();
            return 1;
        }
    };
    if app.settings.mouse {
        let _ = terminal.backend_mut().execute(EnableMouseCapture);
    }
    let _ = terminal.clear();

    // The theme is rebuilt from the settings each frame for the same reason the
    // TUI's is: a border or an accent changed on this screen has to be visible
    // here, immediately, rather than after the next launch.
    let mut theme = Theme::new(app.settings.accent, app.settings.border);
    let code = loop {
        match poll(INPUT_WAIT) {
            // A read that fails leaves the screen up: the user is in the middle
            // of something, and a terminal hiccup is not a reason to throw it
            // away unsaved.
            Ok(true) => match read() {
                Ok(TermEvent::Key(k)) => {
                    if let Some(c) = char_for(k) {
                        crate::tui::settings::key(&mut app, c);
                    }
                }
                Ok(_) => {}
                Err(_) => break 1,
            },
            Ok(false) => {}
            Err(_) => break 1,
        }
        setup.drive(&mut app);
        theme.accent = app.settings.accent;
        theme.border = app.settings.border;
        if terminal
            .draw(|f| {
                crate::tui::settings::render(f, f.area(), &app, &theme);
                crate::tui::colour::apply(f.buffer_mut(), depth);
            })
            .is_err()
        {
            break 1;
        }
        // The screen clears `settings_open` on its own save-and-close.
        if !app.settings_open {
            break 0;
        }
    };

    let _ = terminal.backend_mut().execute(DisableMouseCapture);
    guard.restore();
    // The screen's own `q` has already written the file; this is the belt to its
    // braces, for a window closed some other way.
    if let Some(n) = pending {
        eprintln!("trak: {n}");
    }
    code
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    settings: crate::tui::app::Settings,
    config: Option<crate::config::Loaded>,
    notice: Option<String>,
    first_run: bool,
) -> (i32, Option<crate::config::Config>) {
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
    app.config = config
        .as_ref()
        .map(|l| l.config.clone())
        .unwrap_or_default();
    if let Some(l) = &config {
        app.config = l.config.clone();
        if let Some(recovered) = l.notice() {
            // A corrupt file was renamed and the defaults are in use. Saying so
            // is the whole point of renaming it rather than just ignoring it.
            app.toast(recovered);
        }
    }
    if let Some(n) = notice {
        app.toast(n);
    }
    app.web.connection = connection_at_start(&app.config.spotify.client_id);
    let mut setup = SetupRunner::default();
    let depth = crate::tui::colour::Depth::from_env();
    if first_run {
        app.hint = Some(FIRST_RUN_HINT.to_string());
    }
    // The accent comes off the cover, so the theme is mutable for the life of the
    // session (TODO 4.2).
    let mut theme = Theme::new(settings.accent, settings.border);
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
    // What covered the cover last frame; see the draw step.
    let mut last_covering: Option<(bool, bool, bool, bool)> = None;
    let mut last_sonar = Instant::now() - SONAR_EVERY;
    let mut last_viz = Instant::now() - VIZ_FPS;
    // The real-audio source (TODO 8.3). Created idle: nothing touches Core Audio
    // until the visualizer is on screen, and dropping it at the end of this
    // function joins the worker, which is what takes the tap down on a normal exit.
    let mut audio = crate::audio::AudioPipeline::new(app.settings.visualizer_source);

    // What the user asked for and the worker has not taken yet. The worker runs
    // one job at a time and refuses the rest, and a status poll is in flight a
    // good part of every second -- so a key sent straight to it was thrown away
    // while `app.busy` went on saying a command was running, and from then on
    // every transport key was ignored. Queued here instead, and handed over the
    // moment the worker is free, ahead of any poll.
    let mut pending: std::collections::VecDeque<PlayerCommand> = Default::default();
    let mut pending_web: std::collections::VecDeque<WebJob> = Default::default();

    loop {
        flush(&mut pending, &mut pending_web, &worker);

        // 1. Finished writes first, so a completed command is applied before the
        //    next key can queue another.
        while let Some(result) = worker.poll() {
            app = match result {
                WorkerResult::State(s) => update(app, Event::PlayerState(s)).app,
                // A Web API answer is already an app event, so there is one arm
                // rather than one per tab.
                WorkerResult::Web(event) => update(app, event).app,
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
            && let Some(state) = app.state.as_ref()
        {
            let track_changed = event.is_different_track(state);
            let merged = crate::player::notify::merge(state, &event);
            app = update(app, Event::PlayerState(Box::new(merged))).app;

            // A track change is the one thing the notification cannot be
            // trusted about on its own: it has no artwork, play count or
            // popularity, so one read has to follow. It is cheap, because
            // that is once per song rather than once per poll.
            if track_changed {
                // TODO 4.5. The first read of a session is not a change, and
                // `app.state` is only set after one, so announcing here
                // cannot announce what was already playing when trak started.
                let u = update(app, Event::SoundForTrackChanged);
                app = u.app;
                pending.extend(u.commands);
                worker.submit(|p| match p.state() {
                    Ok(st) => crate::player::actions::WorkerResult::State(Box::new(st)),
                    Err(e) => crate::player::actions::WorkerResult::ReadFailed(e),
                });
            }
        }

        // 2z. The Web API tab that is showing, fetched the first time it is shown
        //     (7.6-7.9). A tab the user has already loaded is not fetched again:
        //     the dev-mode quota is per developer account and shared across Client
        //     IDs, so a refresh that fetches what is already on screen is spent
        //     quota for nothing.
        if app.tab.needs_web()
            && app.web.connection.connected()
            && let Some(job) = tab_needs(app.tab, &app.web)
        {
            submit_web(vec![job], &worker);
        }
        // The playing track's heart (7.7), asked once per track. Skipped while
        // the worker is busy, so a check that could not be sent is not marked as
        // sent and is simply asked on a later pass.
        // The next page of the list on screen (7.9), when the cursor is near the
        // end of it.
        if app.web.connection.connected()
            && !worker.is_busy()
            && let Some(job) = app.web.next_more(app.tab)
            && !submit_web(vec![job], &worker)
        {
            // Not sent, so it is not on its way.
            app.web.loading_more = false;
        }
        if !worker.is_busy() {
            let playing = app.track().and_then(|t| t.uri.clone());
            let connected = app.web.connection.connected();
            if let Some(job) = app.web.next_liked_check(playing.as_deref(), connected) {
                submit_web(vec![job], &worker);
            }
        }
        // A page that has just been opened, and an opened page that has no rows
        // yet. The open page is fetched because the user asked for it by name.
        if let Some(open) = app.web.open.clone()
            && let Some(job) = page_needs(&open, &app.web)
        {
            submit_web(vec![job], &worker);
        }

        // 2b. Album art (TODO 4.1). One download per track, on the worker, and
        //     only when the track has artwork we do not already have. The cache
        //     makes a rewind through the history free.
        if app.settings.show_art
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
                let accepted = worker.submit(move |_| {
                    let album = (!album.is_empty()).then_some(album);
                    let result = crate::lyrics::fetch(&title, &artist, album.as_deref(), Some(dur));
                    WorkerResult::Lyrics { uri, result }
                });
                // `submit` drops a job while another is in flight, and the
                // status was claimed above: leaving it `Loading` after a refusal
                // is how every track said "looking for lyrics…" forever.
                if !accepted {
                    app.lyrics.uri = None;
                    app.lyrics.status = crate::tui::app::LyricsStatus::Idle;
                }
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
                        pending.extend(u.commands);
                        pending_web.extend(u.web);
                        flush(&mut pending, &mut pending_web, &worker);
                    }
                }
                Ok(TermEvent::Mouse(m)) => {
                    if app.settings.mouse {
                        let u = update(app, mouse_event(m, &regions, &mut scrubbing));
                        app = u.app;
                        pending.extend(u.commands);
                        pending_web.extend(u.web);
                        flush(&mut pending, &mut pending_web, &worker);
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
        //
        // The wait is shorter while the visualizer is on screen and the same as
        // ever otherwise, because 10 fps is plenty for a dashboard and is a
        // tenth of the wake-ups (TODO 11.5's idle-CPU budget).
        let frame = if visualizer_visible(&app) {
            VIZ_FPS
        } else {
            INPUT_WAIT
        };
        crate::player::notify::pump_run_loop(frame.as_secs_f64());

        // 4. The local tick: toast expiry and the poll-due flag.
        //     It can also fire the debounced search (7.6), so its web jobs are
        //     sent rather than dropped.
        let ticked = update(app, Event::Tick);
        app = ticked.app;
        submit_web(ticked.web, &worker);

        // 4b. The visualizer's own tick. Separate from the clock above because
        //     bars need thirty frames a second and the clock needs one, and
        //     because a tap (TODO 8.3) has to be able to stop without the clock
        //     noticing (TODO 8.5).
        //
        //     The tap follows the same test as the frame rate (TODO 8.5): on
        //     screen and Spotify running means attached, anything else means
        //     released. The setting is re-read every frame because the settings
        //     screen can change it under us.
        audio.set_source(app.settings.visualizer_source);
        audio.set_wanted(visualizer_visible(&app));
        if let Some(line) = audio.take_notice() {
            app = update(app, Event::TapNotice(line)).app;
        }
        if visualizer_visible(&app) && last_viz.elapsed() >= VIZ_FPS {
            last_viz = Instant::now();
            let frame = match audio.live_spectrum() {
                Some(bars) => Event::LiveSpectrum(bars),
                None => Event::VizTick,
            };
            app = update(app, frame).app;
        }

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
        //
        // The theme is rebuilt from the settings every frame rather than once at
        // startup, which is the only reason `,` can preview a border or an accent
        // change on the live dashboard before you close the screen.
        setup.drive(&mut app);
        theme.accent = app.settings.accent;
        theme.border = app.settings.border;
        let settings_open = app.settings_open;
        // A Kitty cover is drawn row by row from each row's first cell, and
        // ratatui-image marks the rest of the row "skip", which ratatui's diff
        // never rewrites. So when something that was drawn over the cover goes
        // away, the cells it left behind stay on top of the picture. Anything
        // that covers the cover therefore forces one full repaint when it opens
        // or closes.
        let covering = (
            settings_open,
            app.setup.open,
            app.lyrics_full,
            app.web.edit.is_some(),
        );
        if last_covering.is_some_and(|c| c != covering) {
            let _ = terminal.clear();
        }
        last_covering = Some(covering);
        if terminal
            .draw(|f| {
                crate::tui::render::draw_with(f, &app, &theme, &mut regions, &mut images);
                if settings_open {
                    crate::tui::settings::render(f, f.area(), &app, &theme);
                }
                // Last, so it sees every cell: NO_COLOR and 16/256-colour
                // terminals are handled once here, not by each widget (11.6).
                crate::tui::colour::apply(f.buffer_mut(), depth);
            })
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

    // The settings are written on the way out rather than on every key, so
    // holding `l` down does not mean thirty writes a second -- and a quit is the
    // one moment every change is definitely meant to stick. The screen's own
    // `q` saves too, because closing it is an explicit "keep these".
    //
    // The terminal is restored first: a save can fail, and a failed save has to be
    // reportable, which needs a terminal.
    let pending = app
        .config_dirty
        .then(|| app.config.with_settings(&app.settings));
    (0, pending)
}

/// What the connection looks like at start, from the config and the token file.
///
/// Without this `app.web.connection` stays at its default for the whole session
/// and no Web API tab can ever load, whatever the token file holds.
fn connection_at_start(client_id: &str) -> crate::tui::app::Connection {
    use crate::tui::app::Connection as Shown;
    use crate::web::token::Connection as Stored;
    if client_id.is_empty() {
        return Shown::NoClientId;
    }
    let store = crate::web::token::TokenFile::at(&crate::config::Paths::from_env());
    let Ok(loaded) = store.load() else {
        return Shown::LoggedOut;
    };
    match Stored::of(&loaded, std::time::SystemTime::now()) {
        Stored::Connected | Stored::ExpiringSoon => Shown::Connected,
        Stored::LoggedOut => Shown::LoggedOut,
        Stored::NeedsReconnect => Shown::NeedsRelogin,
    }
}

/// Carries out what the guided setup panel asked for (TODO 7.3).
///
/// The panel's state machine only *says* what should happen; this is the one
/// place that opens a browser, writes the config or waits on a login. The login
/// runs on its own thread because it blocks until the browser redirects back,
/// which can be minutes -- the UI thread must keep drawing meanwhile.
#[derive(Default)]
struct SetupRunner {
    login: Option<std::sync::mpsc::Receiver<Result<(), String>>>,
}

impl SetupRunner {
    fn drive(&mut self, app: &mut App) {
        use crate::tui::setup::Effect;
        use crate::web::auth::{Browser, MacBrowser};
        for effect in std::mem::take(&mut app.setup.pending) {
            match effect {
                // Applied by `setup::key` before it gets here.
                Effect::ClientId(_) => {}
                Effect::Open(url) => {
                    if MacBrowser.open(&url).is_err() {
                        app.setup.notice = Some(format!("could not open a browser -- go to {url}"));
                    }
                }
                Effect::Copy(text) => {
                    app.setup.notice = Some(if crate::player::actions::copy_to_clipboard(&text) {
                        format!("copied {text}")
                    } else {
                        format!("could not copy -- type it exactly: {text}")
                    });
                }
                Effect::SaveConfig => {
                    // The settings are folded in as well, so clearing the dirty flag
                    // cannot lose a toggle made earlier on the same screen.
                    let next = app.config.with_settings(&app.settings);
                    match next.save() {
                        Ok(()) => {
                            app.config = next;
                            app.config_dirty = false;
                        }
                        Err(e) => app.setup.notice = Some(e.notice()),
                    }
                }
                Effect::Login(id) => {
                    if self.login.is_none() {
                        self.login = Some(spawn_login(id));
                    }
                }
                Effect::Logout => {
                    let store = crate::web::token::TokenFile::at(&crate::config::Paths::from_env());
                    match store.clear() {
                        Ok(()) => {
                            app.setup.logged_out();
                            app.web.connection = crate::tui::app::Connection::LoggedOut;
                        }
                        Err(e) => app.setup.notice = Some(e.notice()),
                    }
                }
            }
        }
        if let Some(rx) = &self.login {
            let done = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("trak: the Spotify login stopped unexpectedly".into()))
                }
            };
            if let Some(result) = done {
                self.login = None;
                if result.is_ok() {
                    app.web.connection = crate::tui::app::Connection::Connected;
                }
                app.setup.login_finished(result);
            }
        }
    }
}

fn spawn_login(client_id: String) -> std::sync::mpsc::Receiver<Result<(), String>> {
    use crate::web::auth::{Login, MacBrowser, SpotifyEndpoint};
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let store = crate::web::token::TokenFile::at(&crate::config::Paths::from_env());
        let endpoint = SpotifyEndpoint::default();
        let result = Login::new(&endpoint, &MacBrowser, &store)
            .run(
                &client_id,
                crate::tui::setup::SCOPES,
                std::time::SystemTime::now(),
            )
            .map(|_| ())
            .map_err(|e| e.notice());
        let _ = tx.send(result);
    });
    rx
}

/// Whether there is a token the Web API could be called with. Reads one small
/// file and does no network, so the UI thread may ask it every frame.
///
/// A stale *access* token still counts: it is renewed by [`web_client`] on the
/// worker. An expired *refresh* token is the one case that is not "just stale":
/// it is a new login.
fn web_ready() -> bool {
    let store = crate::web::token::TokenFile::at(&crate::config::Paths::from_env());
    store
        .load()
        .ok()
        .and_then(|l| l.token)
        .is_some_and(|t| !t.refresh_stale(std::time::SystemTime::now()))
}

/// A Web API client for whatever the token file currently holds, or `None` when
/// there is nothing to talk to. **Blocks on the network when the access token is
/// stale**, so it is only for the worker; the UI thread asks [`web_ready`].
///
/// Read fresh each time rather than cached: the token can expire under a running
/// session, and a client holding a dead token is how "search silently stopped
/// working an hour ago" happens. A stale access token is renewed and written back
/// here, which is what keeps a login from lasting exactly one hour.
fn web_client() -> Option<SpotifyLibrary> {
    use crate::web::auth::{Session, SpotifyEndpoint};
    let store = crate::web::token::TokenFile::at(&crate::config::Paths::from_env());
    let token = store.load().ok()?.token?;
    let now = std::time::SystemTime::now();
    if token.refresh_stale(now) {
        return None;
    }
    let token = if token.access_stale(now) {
        let client_id = crate::config::Config::load().ok()?.config.spotify.client_id;
        let endpoint = SpotifyEndpoint::default();
        // A refresh that fails leaves `None` here, so the job is dropped rather
        // than sent with a token Spotify will answer 401.
        Session::new(&endpoint, &store, &client_id)
            .access(&token, now)
            .token()?
            .clone()
    } else {
        token
    };
    Some(SpotifyLibrary::at(crate::web::api::API_BASE, &token))
}

/// Run one Web API job on the worker and turn its answer into an app event.
///
/// Every arm produces an already-built [`Event`], so the loop has one line here
/// and adding a tab adds no arm to its match.
fn run_web(job: WebJob) -> Option<crate::player::actions::WorkerResult> {
    use crate::player::actions::WorkerResult;
    use crate::tui::app::{Event, PageLoaded, PageWhat};
    let client = web_client()?;
    let event = match job {
        WebJob::Search(query) => Event::Searched {
            for_query: query.clone(),
            result: client.search(&query),
        },
        WebJob::Playlists => Event::Page {
            what: PageWhat::Playlists,
            result: client.playlists(None).map(PageLoaded::Playlists),
        },
        WebJob::Liked => Event::Page {
            what: PageWhat::Liked,
            result: client.liked_tracks(None).map(PageLoaded::Liked),
        },
        WebJob::Queue => Event::Queue(client.queue()),
        WebJob::Library(section) => {
            let what = match section {
                crate::tui::app::LibrarySection::Albums => PageWhat::LibraryAlbums,
                crate::tui::app::LibrarySection::Artists => PageWhat::LibraryArtists,
                crate::tui::app::LibrarySection::Recent => PageWhat::LibraryRecent,
            };
            Event::Page {
                what,
                result: match section {
                    crate::tui::app::LibrarySection::Albums => {
                        client.saved_albums(None).map(PageLoaded::LibraryAlbums)
                    }
                    crate::tui::app::LibrarySection::Artists => client
                        .followed_artists(None)
                        .map(PageLoaded::LibraryArtists),
                    crate::tui::app::LibrarySection::Recent => {
                        client.recently_played(None).map(PageLoaded::LibraryRecent)
                    }
                },
            }
        }
        WebJob::More(what, after) => {
            let result = match &what {
                PageWhat::Playlists => client.playlists(Some(&after)).map(PageLoaded::Playlists),
                PageWhat::Liked => client.liked_tracks(Some(&after)).map(PageLoaded::Liked),
                PageWhat::LibraryAlbums => client
                    .saved_albums(Some(&after))
                    .map(PageLoaded::LibraryAlbums),
                PageWhat::LibraryArtists => client
                    .followed_artists(Some(&after))
                    .map(PageLoaded::LibraryArtists),
                PageWhat::LibraryRecent => client
                    .recently_played(Some(&after))
                    .map(PageLoaded::LibraryRecent),
                // Only the five top-level lists are paged.
                _ => return None,
            };
            Event::MorePage { what, result }
        }
        WebJob::PlaylistItems(id) => Event::Page {
            what: PageWhat::PlaylistItems(id.clone()),
            result: client
                .playlist_items(&id, None)
                .map(|page| PageLoaded::PlaylistItems { id, page }),
        },
        WebJob::ArtistAlbums(id) => Event::Page {
            what: PageWhat::ArtistAlbums(id.clone()),
            result: client
                .artist_albums(&id, None)
                .map(|page| PageLoaded::ArtistAlbums { id, page }),
        },
        WebJob::AlbumTracks(id) => Event::Page {
            what: PageWhat::AlbumTracks(id.clone()),
            result: client
                .album_tracks(&id, None)
                .map(|page| PageLoaded::AlbumTracks { id, page }),
        },
        WebJob::Enqueue(uri) => {
            let result = client.enqueue(&uri);
            return Some(WorkerResult::Web(Event::WebWrote(result)));
        }
        WebJob::Like(uri, liked) => {
            let result = client.set_liked(&uri, liked);
            return Some(WorkerResult::Web(Event::WebWrote(result)));
        }
        WebJob::Unlike(uri) => {
            let result = client.set_liked(&uri, false);
            return Some(WorkerResult::Web(Event::WebWrote(result)));
        }
        // Nothing to send: the check is folded into the like write, which is
        // one request rather than two.
        WebJob::IsLiked(uri) => {
            return Some(WorkerResult::Web(Event::LikedHere {
                result: client.is_liked(&uri),
                uri,
            }));
        }
        WebJob::CreatePlaylist(name) => {
            let result = client.create_playlist(&name, false);
            return Some(WorkerResult::Web(Event::WebWrote(result.map(|_| ()))));
        }
        WebJob::AddToPlaylist { playlist, uri } => {
            let result = client.add_to_playlist(&playlist, &uri);
            return Some(WorkerResult::Web(Event::WebWrote(result)));
        }
        WebJob::RemoveFromPlaylist { playlist, uri } => {
            let result = client.remove_from_playlist(&playlist, &uri);
            return Some(WorkerResult::Web(Event::WebWrote(result)));
        }
    };
    Some(WorkerResult::Web(event))
}

/// Queue Web API jobs on the worker.
///
/// One at a time, and only if the worker is free: the same rule as player
/// commands, for the same reason. A refused job is not queued for later either --
/// a search the user has already typed past is not worth sending, and a list they
/// have left is not worth loading.
/// `false` if any job was not sent, so a caller that marked something "on its
/// way" can unmark it.
fn submit_web(jobs: Vec<WebJob>, worker: &Worker) -> bool {
    let mut all_sent = true;
    for job in jobs {
        // `run_web` needs a client, which needs a token. Without one there is
        // nothing to send and the tab has already said so in its own body.
        if !web_ready() {
            all_sent = false;
            continue;
        }
        // `run_web` answers `None` when it has nothing to send -- no client (a
        // refresh that failed) or a job with no request in it. The worker has to
        // answer with *something*: a page that was asked for gets its own error
        // so its "on its way" flag is cleared, and anything else gets `Resize`,
        // which changes nothing. (Not a landed write: that event re-asks the
        // like check, and a failing client would then ask every frame.)
        let fallback = match &job {
            WebJob::More(what, _) => crate::tui::app::Event::MorePage {
                what: what.clone(),
                result: Err(crate::web::api::ApiError::NotConnected),
            },
            _ => crate::tui::app::Event::Resize,
        };
        let accepted = worker.submit(move |_| {
            run_web(job).unwrap_or(crate::player::actions::WorkerResult::Web(fallback))
        });
        if !accepted {
            return false;
        }
    }
    all_sent
}

/// What a tab needs the first time it is shown, if anything.
///
/// Lazily, one list at a time. The Library tab has three lists and most visits
/// leave after looking at the first, so asking for all three on entry would be
/// three requests for nothing -- and the quota is per developer account.
fn tab_needs(tab: Tab, web: &crate::tui::app::WebState) -> Option<WebJob> {
    match tab {
        Tab::Search if !web.searching && web.search_shown.is_empty() => None,
        Tab::Playlists if web.playlists.items.is_empty() && web.playlists.next.is_none() => {
            Some(WebJob::Playlists)
        }
        Tab::Liked if web.liked.items.is_empty() && web.liked.next.is_none() => Some(WebJob::Liked),
        Tab::Queue if web.queue.upcoming.is_empty() && web.queue.now_playing.is_none() => {
            Some(WebJob::Queue)
        }
        Tab::Library => {
            let i = match web.library.section {
                crate::tui::app::LibrarySection::Albums => 0,
                crate::tui::app::LibrarySection::Artists => 1,
                crate::tui::app::LibrarySection::Recent => 2,
            };
            (!web.library.loaded[i]).then_some(WebJob::Library(web.library.section))
        }
        // A page that has just been opened is the thing to fetch, and it is the
        // loop's business because only it knows whether the worker is free.
        _ => None,
    }
}

/// The fetch an open page needs, if it does not have its rows yet.
fn page_needs(open: &crate::tui::app::Open, web: &crate::tui::app::WebState) -> Option<WebJob> {
    match open {
        // Empty rows is the signal: a playlist with no tracks and one that has
        // not been fetched are the same to this code and only one of them is
        // worth a request. The cost is a second request for a genuinely empty
        // playlist, which is cheaper than being wrong about every real one.
        crate::tui::app::Open::Playlist(id) if web.open_tracks.is_empty() => {
            Some(WebJob::PlaylistItems(id.clone()))
        }
        crate::tui::app::Open::Artist(id) if web.open_albums.is_empty() => {
            Some(WebJob::ArtistAlbums(id.clone()))
        }
        crate::tui::app::Open::Album(id) if web.open_track_page.is_empty() => {
            Some(WebJob::AlbumTracks(id.clone()))
        }
        _ => None,
    }
}

/// Hand the user's queued commands, then their Web jobs, to the worker, as many
/// as it takes (one, when it is free).
fn flush(
    pending: &mut std::collections::VecDeque<PlayerCommand>,
    pending_web: &mut std::collections::VecDeque<WebJob>,
    worker: &Worker,
) {
    while let Some(cmd) = pending.front() {
        if !submit_command(cmd.clone(), worker) {
            return;
        }
        pending.pop_front();
    }
    while let Some(job) = pending_web.front() {
        // With no token there is nothing to send, and never will be until the
        // user logs in; the tab already says so.
        if !web_ready() {
            pending_web.clear();
            return;
        }
        if !submit_web(vec![job.clone()], worker) {
            return;
        }
        pending_web.pop_front();
    }
}

/// Offer one command to the worker; `false` if it was busy and did not take it.
fn submit_command(cmd: PlayerCommand, worker: &Worker) -> bool {
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
    })
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
        // AppleScript cannot tell "repeat one" from "repeat all", so the app
        // remembers the mode and this writes the boolean that mode implies.
        // Flipping whatever Spotify had instead put off/all/one out of step:
        // "one" switched repeat off, and "off" switched it back on.
        PlayerCommand::SetRepeating(on) => {
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
        PlayerCommand::Notify(title, body) => {
            // `display notification`, and nothing else: no icon, no sound, and no
            // subtitle, because the point is a quiet line when the track changes.
            let script = format!(
                "display notification \"{}\" with title \"{}\"",
                // A quote or a backslash in a track title would close the string
                // literal, and this is the same allow-list problem `play track`
                // has. AppleScript escapes them the other way round: the quote is
                // the only thing that needs doubling.
                body.replace('"', "\\\""),
                title.replace('"', "\\\"")
            );
            p.command(&script).map(|_| None)
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
/// `↑`/`↓` are folded onto `j`/`k` so both work from one binding (SPEC §4).
/// `←`/`→` switch tabs while `h`/`l` seek, so they get sentinels of their own.
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
        // Backspace arrives as a key, not a character, and the settings screen's
        // one text field (TODO 5.2) needs it as a character so it does not have
        // to be the one place that knows about key codes. Either erase works:
        // macOS terminals send \x7f, the DEC/PC set sends \x08.
        KeyCode::Backspace => Some('\x7f'),
        KeyCode::Left => Some(crate::tui::app::ARROW_LEFT),
        KeyCode::Right => Some(crate::tui::app::ARROW_RIGHT),
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

    /// SPEC §4: `↑`/`↓` are `k`/`j`; `←`/`→` are tab keys of their own, not
    /// `h`/`l`, which seek.
    #[test]
    fn arrows_fold_onto_their_vim_equivalents() {
        assert_eq!(
            char_for(key(KeyCode::Left, KeyModifiers::NONE)),
            Some(crate::tui::app::ARROW_LEFT)
        );
        assert_eq!(
            char_for(key(KeyCode::Right, KeyModifiers::NONE)),
            Some(crate::tui::app::ARROW_RIGHT)
        );
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

    /// A key pressed while the worker is busy with a poll must still reach
    /// Spotify once the poll is done. It used to be dropped, which left
    /// `app.busy` set for good and every transport key dead after it.
    #[test]
    fn a_command_queued_behind_a_busy_worker_still_runs() {
        use crate::player::FakePlayer;
        use std::sync::mpsc::channel;
        let worker = Worker::new(FakePlayer::playing());
        let (release, wait) = channel::<()>();
        // A "poll" that holds the worker until the test lets it go.
        assert!(worker.submit(move |_| {
            let _ = wait.recv();
            WorkerResult::Command(CommandOutcome::ok(PlayerCommand::Toggle))
        }));
        let mut pending: std::collections::VecDeque<PlayerCommand> =
            [PlayerCommand::Next].into_iter().collect();
        let mut pending_web = Default::default();
        flush(&mut pending, &mut pending_web, &worker);
        assert_eq!(pending.len(), 1, "the worker is busy, so it stays queued");
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut ran = false;
        while Instant::now() < deadline && !ran {
            flush(&mut pending, &mut pending_web, &worker);
            if let Some(WorkerResult::Command(out)) = worker.poll()
                && out.cmd == PlayerCommand::Next
            {
                ran = true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(ran, "the queued command never ran");
        assert!(pending.is_empty());
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
                        PlayerCommand::ToggleShuffle | PlayerCommand::SetRepeating(_)
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
